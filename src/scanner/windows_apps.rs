//! Windows 应用扫描器
//!
//! 通过 Windows 注册表扫描已安装应用，支持：
//! - App 缓存扫描（%LOCALAPPDATA%/%APPDATA% 下的 Cache 目录）
//! - App 数据扫描（%LOCALAPPDATA%/%APPDATA% 下的应用数据目录）
//! - App 卸载扫描（注册表 Uninstall 键 + UWP 应用）
//!
//! 注册表位置：
//! - HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall (系统级 64位)
//! - HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall (系统级 32位)
//! - HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall (用户级)

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use super::residual_match::{
    display_name_variants, is_deletable_fs_path, is_deletable_reg_key_basic, is_residual_dir,
    is_residual_reg_key, resembles_app_dir,
};
use super::{dir_size, home_dir, Recommend, ScanItem, ScanResult, Scanner};

// =========================================================================
//  UWP 预装应用数据库（bloatware 列表）
//
//  参考 Win11Debloat (https://github.com/Raphire/Win11Debloat) Config/Apps.json
//  收录 50+ 个常见预装 UWP 应用，带友好名称、推荐等级、卸载方式。
//  用于在 App 卸载 Tab 中高亮显示可安全清理的 bloatware。
// =========================================================================

/// UWP 预装应用条目
#[derive(Debug, Clone)]
pub struct BloatwareEntry {
    /// 友好显示名称
    pub friendly_name: &'static str,
    /// UWP 包名或 AppId（用于匹配 Get-AppxPackage / WinGet）
    pub app_id: &'static str,
    /// 描述
    pub description: &'static str,
    /// 是否默认选中（对应 Win11Debloat 的 SelectedByDefault）
    pub selected_by_default: bool,
    /// 推荐等级（对应 maclean 的 Recommend）
    pub recommend: Recommend,
    /// 卸载方式：Appx / WinGet / AppxOrWinGet
    pub removal_method: RemovalMethod,
}

/// 卸载方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RemovalMethod {
    /// 通过 Remove-AppxPackage 卸载
    Appx,
    /// 通过 winget uninstall 卸载
    WinGet,
    /// 优先 Appx，失败则尝试 WinGet
    AppxOrWinGet,
}

// =========================================================================
//  P2-1: 动态 bloatware 数据库（用户 apps.json 可覆盖内置 DB）
// =========================================================================

/// 可序列化的 bloatware 条目（owned 版本，兼容 Win11Debloat Apps.json 格式）
///
/// JSON 示例:
/// ```json
/// {
///   "FriendlyName": "Clipchamp",
///   "AppId": "Clipchamp.Clipchamp",
///   "Description": "Video editor from Microsoft",
///   "SelectedByDefault": true,
///   "Recommendation": "safe",
///   "RemovalMethod": "Appx"
/// }
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BloatwareEntryOwned {
    pub friendly_name: String,
    pub app_id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub selected_by_default: bool,
    #[serde(
        rename = "Recommendation",
        with = "recommend_serde",
        default = "default_recommend"
    )]
    pub recommend: Recommend,
    #[serde(default = "default_removal_method")]
    pub removal_method: RemovalMethod,
}

fn default_recommend() -> Recommend {
    Recommend::Safe
}

fn default_removal_method() -> RemovalMethod {
    RemovalMethod::Appx
}

/// Recommend 与 Win11Debloat 字符串格式的互转
///
/// JSON: "safe" | "caution" | "advanced" | "cacheonly"（大小写不敏感）
mod recommend_serde {
    use super::Recommend;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(r: &Recommend, s: S) -> Result<S::Ok, S::Error> {
        let text = match r {
            Recommend::Safe => "safe",
            Recommend::CacheOnly => "cacheonly",
            Recommend::Caution => "caution",
            Recommend::Advanced => "advanced",
        };
        s.serialize_str(text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Recommend, D::Error> {
        let text = String::deserialize(d)?.to_lowercase();
        Ok(match text.as_str() {
            "safe" => Recommend::Safe,
            "cacheonly" | "cache_only" | "cache" => Recommend::CacheOnly,
            "caution" => Recommend::Caution,
            "advanced" => Recommend::Advanced,
            _ => Recommend::Safe,
        })
    }
}

/// Win11Debloat Apps.json 顶层结构
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AppsJsonFile {
    #[serde(default)]
    #[allow(dead_code)]
    version: String,
    apps: Vec<BloatwareEntryOwned>,
}

/// 内置 DB 转 owned 版本
fn builtin_entries_owned() -> Vec<BloatwareEntryOwned> {
    BLOATWARE_DB
        .iter()
        .map(|e| BloatwareEntryOwned {
            friendly_name: e.friendly_name.to_string(),
            app_id: e.app_id.to_string(),
            description: e.description.to_string(),
            selected_by_default: e.selected_by_default,
            recommend: e.recommend,
            removal_method: e.removal_method,
        })
        .collect()
}

/// 加载 bloatware 条目（用户 apps.json 优先，内置 DB 兜底）
fn load_bloatware_entries() -> Vec<BloatwareEntryOwned> {
    let user_path = crate::config::apps_json_path();
    if let Ok(content) = std::fs::read_to_string(&user_path) {
        match serde_json::from_str::<AppsJsonFile>(&content) {
            Ok(cfg) if !cfg.apps.is_empty() => {
                crate::logger::info(&format!(
                    "已加载用户 bloatware 配置 ({}): {} 项",
                    user_path.display(),
                    cfg.apps.len()
                ));
                return cfg.apps;
            }
            Ok(_) => {
                crate::logger::error("用户 apps.json 为空，回退内置数据库");
            }
            Err(e) => {
                crate::logger::error(&format!("用户 apps.json 解析失败（回退内置）: {}", e));
            }
        }
    }
    builtin_entries_owned()
}

/// 全局 bloatware 数据库（启动时加载一次）
static BLOATWARE_CACHE: std::sync::OnceLock<Vec<BloatwareEntryOwned>> = std::sync::OnceLock::new();

/// 获取 bloatware 数据库引用
pub fn bloatware_db() -> &'static [BloatwareEntryOwned] {
    BLOATWARE_CACHE.get_or_init(load_bloatware_entries)
}

/// 内置 bloatware 数据库（50+ 项）
///
/// 按类别分组：Microsoft 系 → OEM/第三方 → 游戏/娱乐 → 工具
pub const BLOATWARE_DB: &[BloatwareEntry] = &[
    // ===== Microsoft 系 =====
    BloatwareEntry {
        friendly_name: "Clipchamp",
        app_id: "Clipchamp.Clipchamp",
        description: "Video editor from Microsoft",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "3D Builder",
        app_id: "Microsoft.3DBuilder",
        description: "Basic 3D modeling software",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Cortana",
        app_id: "Microsoft.549981C3F5F10",
        description: "Microsoft Cortana voice assistant (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing Finance",
        app_id: "Microsoft.BingFinance",
        description: "Finance news and tracking via Bing (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing Food And Drink",
        app_id: "Microsoft.BingFoodAndDrink",
        description: "Recipes and food news via Bing (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing Health And Fitness",
        app_id: "Microsoft.BingHealthAndFitness",
        description: "Health and fitness tracking/news via Bing (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing News",
        app_id: "Microsoft.BingNews",
        description: "News aggregator via Bing (Replaced by Microsoft News/Start)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing Sports",
        app_id: "Microsoft.BingSports",
        description: "Sports news and scores via Bing (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing Translator",
        app_id: "Microsoft.BingTranslator",
        description: "Translation service via Bing",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing Travel",
        app_id: "Microsoft.BingTravel",
        description: "Travel planning and news via Bing (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Bing Weather",
        app_id: "Microsoft.BingWeather",
        description: "Weather forecast via Bing",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft Copilot",
        app_id: "XP9CXNGPPJ97XX",
        description: "AI assistant integrated into Windows",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::WinGet,
    },
    BloatwareEntry {
        friendly_name: "Copilot+ AI Hub",
        app_id: "Microsoft.Windows.AIHub",
        description: "Copilot+ AI Hub app (Windows 11 24H2+)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft PC Manager",
        app_id: "Microsoft.PCManager",
        description: "Microsoft PC Manager system cleanup tool",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Get Started",
        app_id: "Microsoft.Getstarted",
        description: "Tips and introductory guide for Windows",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Messaging",
        app_id: "Microsoft.Messaging",
        description: "Messaging app (Largely discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "3D Viewer",
        app_id: "Microsoft.Microsoft3DViewer",
        description: "Viewer for 3D models",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft Journal",
        app_id: "Microsoft.MicrosoftJournal",
        description: "Digital note-taking app optimized for pen input",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Office Hub",
        app_id: "Microsoft.MicrosoftOfficeHub",
        description: "Hub to access Microsoft Office apps and documents",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Power BI",
        app_id: "Microsoft.MicrosoftPowerBIForWindows",
        description: "Business analytics service client",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Solitaire Collection",
        app_id: "Microsoft.MicrosoftSolitaireCollection",
        description: "Collection of solitaire card games",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Sticky Notes",
        app_id: "Microsoft.MicrosoftStickyNotes",
        description: "Digital sticky notes app (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Mixed Reality Portal",
        app_id: "Microsoft.MixedReality.Portal",
        description: "Portal for Windows Mixed Reality headsets",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Network Speed Test",
        app_id: "Microsoft.NetworkSpeedTest",
        description: "Internet connection speed test utility",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft News",
        app_id: "Microsoft.News",
        description: "News aggregator (now part of Microsoft Start)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "OneNote",
        app_id: "Microsoft.Office.OneNote",
        description: "Digital note-taking app (UWP version)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Sway",
        app_id: "Microsoft.Office.Sway",
        description: "Presentation and storytelling app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "One Connect",
        app_id: "Microsoft.OneConnect",
        description: "Mobile Operator management app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Print 3D",
        app_id: "Microsoft.Print3D",
        description: "3D printing preparation software",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Power Automate",
        app_id: "Microsoft.PowerAutomateDesktop",
        description: "Desktop automation tool (RPA)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Skype (UWP)",
        app_id: "Microsoft.SkypeApp",
        description: "Skype communication app (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft To Do",
        app_id: "Microsoft.Todos",
        description: "To-do list and task management app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Dev Home",
        app_id: "Microsoft.Windows.DevHome",
        description: "Developer dashboard (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Alarms & Clock",
        app_id: "Microsoft.WindowsAlarms",
        description: "Alarms & Clock app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Feedback Hub",
        app_id: "Microsoft.WindowsFeedbackHub",
        description: "App for providing feedback to Microsoft",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Windows Maps",
        app_id: "Microsoft.WindowsMaps",
        description: "Mapping and navigation app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Sound Recorder",
        app_id: "Microsoft.WindowsSoundRecorder",
        description: "Basic audio recording app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Xbox Console Companion",
        app_id: "Microsoft.XboxApp",
        description: "Old Xbox Console Companion App (Discontinued)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Movies & TV",
        app_id: "Microsoft.ZuneVideo",
        description: "Movies & TV app (Films & TV)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Family Safety",
        app_id: "MicrosoftCorporationII.MicrosoftFamily",
        description: "Family Safety App for managing family accounts",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Quick Assist",
        app_id: "MicrosoftCorporationII.QuickAssist",
        description: "Remote assistance tool",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft Teams (Old)",
        app_id: "MicrosoftTeams",
        description: "Old Microsoft Teams personal (MS Store)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft Teams (New)",
        app_id: "MSTeams",
        description: "New Microsoft Teams app (Work/School/Personal)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    // ===== OEM/第三方 =====
    BloatwareEntry {
        friendly_name: "ACG Media Player",
        app_id: "ACGMediaPlayer",
        description: "Media player app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Actipro Software",
        app_id: "ActiproSoftwareLLC",
        description: "UI controls or software components, often bundled by OEMs",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Adobe Photoshop Express",
        app_id: "AdobeSystemsIncorporated.AdobePhotoshopExpress",
        description: "Basic photo editing app from Adobe",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Amazon",
        app_id: "Amazon.com.Amazon",
        description: "Amazon shopping app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Prime Video",
        app_id: "AmazonVideo.PrimeVideo",
        description: "Amazon Prime Video streaming service app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Autodesk SketchBook",
        app_id: "AutodeskSketchBook",
        description: "Digital drawing and sketching app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Candy Crush Saga",
        app_id: "CandyCrushSaga",
        description: "Popular puzzle game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Candy Crush Soda Saga",
        app_id: "CandyCrushSodaSaga",
        description: "Puzzle game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Disney Magic Kingdoms",
        app_id: "DisneyMagicKingdoms",
        description: "Disney-themed simulation game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Dolby Access",
        app_id: "DolbyLaboratories.DolbyAccess",
        description: "Dolby Atmos sound enhancement app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Dolby Audio",
        app_id: "DolbyLaboratories.DolbyAudio",
        description: "Dolby audio driver companion app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Dragon Mania Legends",
        app_id: "DragonManiaLegends",
        description: "Dragon breeding simulation game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Farm Heroes Saga",
        app_id: "FarmHeroesSaga",
        description: "Puzzle game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Hidden City",
        app_id: "HiddenCityMysteryofShadows",
        description: "Hidden object adventure game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "iHeartRadio",
        app_id: "iHeartRadio",
        description: "Radio and music streaming app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "LinkedIn",
        app_id: "LinkedInforWindows",
        description: "Professional networking app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "March of Empires",
        app_id: "MarchofEmpires",
        description: "Strategy game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Minecraft",
        app_id: "Minecraft",
        description: "Minecraft for Windows 10/11 (Bedrock Edition)",
        selected_by_default: true,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Netflix",
        app_id: "Netflix",
        description: "Streaming service app",
        selected_by_default: true,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Pandora",
        app_id: "PandoraMediaInc",
        description: "Music streaming app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Phototastic Collage",
        app_id: "PhototasticCollage",
        description: "Photo collage maker",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "PicsArt Photo Studio",
        app_id: "PicsArtPhotoStudio",
        description: "Photo editing app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Royal Revolt 2",
        app_id: "RoyalRevolt2",
        description: "Strategy game",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Shazam",
        app_id: "Shazam",
        description: "Music recognition app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Snapchat",
        app_id: "Snapchat",
        description: "Social media app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Speed Test by Ookla",
        app_id: "Ookla.Speedtest",
        description: "Internet speed test app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Spotify",
        app_id: "SpotifyAB.SpotifyMusic",
        description: "Music streaming app",
        selected_by_default: true,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "TikTok",
        app_id: "TikTok",
        description: "Short video app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "TripAdvisor",
        app_id: "TripAdvisor",
        description: "Travel reviews app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Twitter",
        app_id: "Twitter",
        description: "Social media app (X)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Viber",
        app_id: "Viber",
        description: "Messaging and calling app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Wikipedia",
        app_id: "Wikipedia",
        description: "Wikipedia app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Wunderlist",
        app_id: "Wunderlist",
        description: "To-do list app (Discontinued, replaced by Microsoft To Do)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Xbox Game Bar",
        app_id: "Microsoft.XboxGamingOverlay",
        description: "Game bar overlay for recording and sharing",
        selected_by_default: false,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Xbox Game Speech Window",
        app_id: "Microsoft.XboxSpeechToTextOverlay",
        description: "Speech-to-text overlay for gaming",
        selected_by_default: false,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Xbox Identity Provider",
        app_id: "Microsoft.XboxIdentityProvider",
        description: "Xbox authentication provider",
        selected_by_default: false,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Your Phone",
        app_id: "Microsoft.YourPhone",
        description: "Phone Link companion app",
        selected_by_default: false,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Groove Music",
        app_id: "Microsoft.ZuneMusic",
        description: "Music player (Discontinued, replaced by Media Player)",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Mail and Calendar",
        app_id: "microsoft.windowscommunicationsapps",
        description: "Built-in mail and calendar app",
        selected_by_default: false,
        recommend: Recommend::Caution,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft People",
        app_id: "Microsoft.People",
        description: "Contacts management app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Microsoft Edge",
        app_id: "Microsoft.MicrosoftEdge",
        description: "Web browser (Stable)",
        selected_by_default: false,
        recommend: Recommend::Advanced,
        removal_method: RemovalMethod::AppxOrWinGet,
    },
    BloatwareEntry {
        friendly_name: "Microsoft Edge Dev",
        app_id: "Microsoft.MicrosoftEdgeDevToolsClient",
        description: "Edge developer tools",
        selected_by_default: false,
        recommend: Recommend::Advanced,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Paint 3D",
        app_id: "Microsoft.MSPaint",
        description: "3D painting app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Voice Recorder",
        app_id: "Microsoft.WindowsSoundRecorder",
        description: "Voice recording app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
    BloatwareEntry {
        friendly_name: "Weather",
        app_id: "Microsoft.BingWeather",
        description: "Weather app",
        selected_by_default: true,
        recommend: Recommend::Safe,
        removal_method: RemovalMethod::Appx,
    },
];

// =========================================================================
//  磁盘扫描配置
//
//  默认只扫描 C 盘（大部分应用安装在 C 盘）。
//  用户可通过 set_scan_all_disks(true) 开启扫描全部磁盘。
//  注册表扫描始终覆盖所有盘符的应用（UninstallString 含完整路径），
//  此配置仅影响 Program Files 直接目录扫描。
// =========================================================================

static SCAN_ALL_DISKS: AtomicBool = AtomicBool::new(false);

/// 设置是否扫描全部磁盘
pub fn set_scan_all_disks(enabled: bool) {
    SCAN_ALL_DISKS.store(enabled, Ordering::Relaxed);
    crate::logger::info(&format!(
        "磁盘扫描配置: {}",
        if enabled { "全部磁盘" } else { "仅 C 盘" }
    ));
}

/// 获取当前磁盘扫描配置
pub fn should_scan_all_disks() -> bool {
    SCAN_ALL_DISKS.load(Ordering::Relaxed)
}

/// 列出系统可用的磁盘盘符
pub fn list_available_disks() -> Vec<char> {
    let mut disks = vec!['C'];
    for drive in ['D', 'E', 'F', 'G', 'H', 'I', 'J'] {
        let path = format!("{}:\\", drive);
        if Path::new(&path).exists() {
            disks.push(drive);
        }
    }
    disks
}

/// Windows 已安装应用信息
#[derive(Debug, Clone)]
pub struct WindowsAppInfo {
    /// 显示名称
    pub name: String,
    /// 安装路径
    pub install_location: Option<String>,
    /// 卸载命令
    pub uninstall_string: Option<String>,
    /// 静默卸载命令
    pub quiet_uninstall_string: Option<String>,
    /// 估计大小（KB）
    pub estimated_size: Option<u64>,
    /// 发布者
    pub publisher: Option<String>,
    /// 注册表键名（ProductCode 或唯一标识）
    pub key_name: String,
    /// 是否为 UWP/Store 应用
    pub is_uwp: bool,
    /// 注册表 SystemComponent 标志（=1 表示系统组件，Windows 自带的隐藏标记）
    pub system_component: bool,
}

// =========================================================================
//  应用保护规则（通用检测，非枚举具体应用名）
//
//  设计原则：
//  1. Critical（系统关键）：用注册表 SystemComponent 标志 + 系统安装路径 +
//     Microsoft 发布者 + 系统关键词，不枚举具体应用名
//  2. Security（安全软件）：用厂商关键词（稳定）+ 类别关键词（覆盖未列出厂商）
//  3. DataProtected（数据保护）：用功能类别关键词 + 数据目录大小启发式
//     （任何用户数据 >50MB 的应用自动标记，无需枚举 IM/云盘等应用）
// =========================================================================

/// 系统安装路径前缀（安装在这些目录下的应用视为系统级）
///
/// 注意：不包含裸 "C:\Program Files\"，因为该目录下大部分是普通第三方应用。
/// 只匹配 Microsoft 和 Windows 子目录。
const SYSTEM_INSTALL_PATH_PREFIXES: &[&str] = &[
    "c:\\windows\\",
    "c:\\program files\\windowsapps\\",
    "c:\\program files (x86)\\windowsapps\\",
    "c:\\program files\\microsoft\\",
    "c:\\program files (x86)\\microsoft\\",
    "c:\\program files\\windows\\",
    "c:\\program files (x86)\\windows\\",
    "c:\\programdata\\microsoft\\",
];

/// 系统应用名称关键词（仅当发布者为 Microsoft 时才匹配）
///
/// 这些是功能类别词，不是具体应用名，可覆盖同一类别的所有应用。
const SYSTEM_NAME_KEYWORDS: &[&str] = &[
    "windows ",
    "microsoft .net",
    "microsoft visual c++",
    "microsoft edge",
    "windows defender",
    "windows security",
    "microsoft store",
    "windows installer",
    "windows sdk",
    "windows subsystem",
    "wsl",
    "microsoft azure",
    "directx",
];

/// 安全软件厂商关键词
///
/// 匹配应用名或发布者。厂商名稳定，比应用名少很多
/// （一家厂商可能发布多款安全产品，如 ESET NOD32 / ESET Internet Security）。
const SECURITY_VENDOR_KEYWORDS: &[&str] = &[
    "CrowdStrike",
    "SentinelOne",
    "Sentinel Labs",
    "ESET",
    "Kaspersky",
    "McAfee",
    "Norton",
    "Bitdefender",
    "Trend Micro",
    "Sophos",
    "Carbon Black",
    "Cylance",
    "Trellix",
    "Jamf",
    "Palo Alto",
    "Fortinet",
    "Symantec",
    "Webroot",
    "Malwarebytes",
    "Tanium",
];

/// 安全软件类别关键词（匹配应用名，覆盖未列出的厂商）
const SECURITY_CATEGORY_KEYWORDS: &[&str] = &[
    "antivirus",
    "anti-virus",
    "anti malware",
    "anti-malware",
    "endpoint protection",
    "endpoint security",
    "internet security",
    "total security",
    "firewall",
    "mdm agent",
];

/// 数据保护功能类别关键词（按功能类别，不枚举具体应用名）
///
/// 密码管理器、输入法、认证器等有明确类别词的应用用关键词匹配。
/// IM/云盘等应用通过数据目录大小启发式检测，无需枚举。
const DATA_PROTECTED_KEYWORDS: &[&str] = &[
    // 密码/认证
    "password",
    "authenticator",
    "wallet",
    "vault",
    "keepass",
    // 输入法
    "输入法",
    "input method",
    "小狼毫",
    "rime",
];

/// 数据保护的数据目录大小阈值（字节）
///
/// 超过此值的应用数据目录（在 %APPDATA% 或 %LOCALAPPDATA% 下），
/// 视为含有用户重要数据（聊天记录、配置等），标记为 DataProtected。
/// 这使得 IM 应用（微信、QQ、Telegram 等）无需枚举即可被保护。
const DATA_PROTECTED_SIZE_THRESHOLD: u64 = 50 * 1024 * 1024; // 50MB

/// Windows 系统级目录名称列表
///
/// 用于 App 残留扫描时跳过系统/共享目录，避免误删。
/// 名称统一使用小写，通过 `is_windows_system_name` 做不区分大小写匹配。
const WINDOWS_SYSTEM_NAMES: &[&str] = &[
    // 系统目录
    "microsoft",
    "windows",
    "packages",
    "programdata",
    "winsxs",
    "system32",
    "syswow64",
    // 系统组件
    "microsoftedge",
    "microsoftedgeupdate",
    "edge",
    "edgeupdate",
    "office",
    "teams",
    "onedrive",
    // 错误报告
    "crashdumps",
    "crashreports",
    "errorreports",
    "wer",
    "wermgr",
    // 系统服务
    "d3dscache",
    "vault",
    "identitycrl",
    "credentials",
    "wbem",
    "catroot",
    "catroot2",
    "driverstore",
    "drivers",
    "config",
    "logs",
    "fonts",
    "help",
    "ime",
    "inf",
    "migration",
    "debug",
    "diagnostics",
    "serviceprofiles",
    "servicing",
    "speech",
    "speech_onecore",
    "systemapps",
    "systemresources",
    "twain_32",
    "web",
    // 用户配置
    "roaming",
    "local",
    "locallow",
    "usertemplates",
    "network",
    "start menu",
    // 厂商共享目录（删除可能影响同厂商其他产品）
    "adobe",
    "macromedia",
    "common files",
    "common",
    "nvidia",
    "amd",
    "intel",
    "realtek",
    "synaptics",
    "logitech",
    "corsair",
    "razer",
    "steelseries",
    "msi",
    "gigabyte",
    "asrock",
    "creative",
    "dolby",
    "nahimic",
    "bangolufsen",
    "hp",
    "dell",
    "lenovo",
    "asus",
    "acer",
    "huawei",
    "xiaomi",
    "samsung",
    "lg",
    "sony",
];

/// 判断目录名是否为 Windows 系统级目录
fn is_windows_system_name(name: &str) -> bool {
    WINDOWS_SYSTEM_NAMES
        .iter()
        .any(|&sys| name.eq_ignore_ascii_case(sys))
}

/// 应用保护级别
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinProtectionLevel {
    None,
    Critical,
    RequiresOfficialUninstaller,
    DataProtected,
}

/// 检查应用保护级别（通用规则检测）
///
/// 不依赖具体应用名枚举，而是通过：
/// 1. 注册表 SystemComponent 标志 + 系统路径 + Microsoft 发布者 → Critical
/// 2. 厂商关键词 + 安全类别关键词 → RequiresOfficialUninstaller
/// 3. 功能类别关键词 + 数据目录大小启发式 → DataProtected
fn check_protection(app: &WindowsAppInfo) -> WinProtectionLevel {
    // 1. Critical: 系统关键应用
    if is_critical_system_app(app) {
        return WinProtectionLevel::Critical;
    }
    // 2. Security: 需官方卸载工具
    if is_security_app(app) {
        return WinProtectionLevel::RequiresOfficialUninstaller;
    }
    // 3. DataProtected: 数据保护
    if is_data_protected_app(app) {
        return WinProtectionLevel::DataProtected;
    }
    WinProtectionLevel::None
}

/// 判断是否为系统关键应用
///
/// 检测规则（任一命中即视为系统应用）：
/// 1. 注册表 SystemComponent 标志 = 1（Windows 自带的系统组件标记）
/// 2. 安装路径在系统目录下（C:\Windows\、WindowsApps 等）
/// 3. 发布者为 Microsoft 且名称含系统关键词（windows、.net、defender 等）
fn is_critical_system_app(app: &WindowsAppInfo) -> bool {
    // 规则 1: 注册表 SystemComponent 标志
    if app.system_component {
        return true;
    }

    // 规则 2: 安装在系统目录
    if let Some(ref loc) = app.install_location {
        let loc_lower = loc.to_lowercase();
        if SYSTEM_INSTALL_PATH_PREFIXES
            .iter()
            .any(|p| loc_lower.starts_with(p))
        {
            return true;
        }
    }

    // 规则 3: Microsoft 发布 + 系统关键词
    let is_microsoft = app
        .publisher
        .as_deref()
        .map(|p| p.to_lowercase().contains("microsoft"))
        .unwrap_or(false);
    if is_microsoft {
        let name_lower = app.name.to_lowercase();
        if SYSTEM_NAME_KEYWORDS
            .iter()
            .any(|kw| name_lower.contains(kw))
        {
            return true;
        }
    }

    false
}

/// 在 bloatware 数据库中查找匹配的应用
///
/// 匹配策略（按优先级）：
/// 1. 精确匹配 app_id（UWP 的 Name 字段即包名，如 Microsoft.BingWeather）
/// 2. 精确匹配 app_id 与 PackageFullName 的 Name 段（第一个 '_' 之前）
/// 3. 精确匹配 friendly_name（应用显示名，主要命中 Win32 DisplayName）
/// 4. 模糊匹配：应用名/包全名包含 app_id 或 friendly_name
///
/// 数据源为动态 bloatware_db()（用户 apps.json 可覆盖内置 DB）。
/// 返回匹配的 BloatwareEntryOwned 引用，未匹配返回 None。
fn find_bloatware_match(app: &WindowsAppInfo) -> Option<&'static BloatwareEntryOwned> {
    let name_lower = app.name.to_lowercase();
    let key_lower = app.key_name.to_lowercase();
    let db = bloatware_db();

    // 1. 精确匹配 app_id（UWP 扫描时 name 字段存的就是包名）
    for entry in db {
        if entry.app_id.eq_ignore_ascii_case(&app.name) {
            return Some(entry);
        }
    }

    // 2. PackageFullName 的 Name 段匹配（格式: <Name>_<Version>_<Arch>_<ResourceId>_<PublisherId>）
    if app.is_uwp {
        let pkg_name = app.key_name.split('_').next().unwrap_or("");
        if !pkg_name.is_empty() {
            for entry in db {
                if entry.app_id.eq_ignore_ascii_case(pkg_name) {
                    return Some(entry);
                }
            }
        }
    }

    // 3. 精确匹配 friendly_name（应用显示名）
    for entry in db {
        if entry.friendly_name.eq_ignore_ascii_case(&app.name) {
            return Some(entry);
        }
    }

    // 4. 模糊匹配：应用名/包全名包含 app_id 或 friendly_name（处理版本后缀等变体）
    for entry in db {
        let app_id_lower = entry.app_id.to_lowercase();
        let friendly_lower = entry.friendly_name.to_lowercase();

        // 应用名包含 app_id（如 "Microsoft.BingWeather_8wekyb3d8bbwe"）
        if key_lower.contains(&app_id_lower) {
            return Some(entry);
        }
        // 应用名包含 friendly_name（如 "Microsoft Copilot" 匹配 "Microsoft Copilot"）
        if name_lower.contains(&friendly_lower) {
            return Some(entry);
        }
    }

    None
}

/// 判断是否为安全/MDM 软件
///
/// 检测规则（任一命中即视为安全软件）：
/// 1. 应用名或发布者包含已知安全厂商关键词
/// 2. 应用名包含安全类别关键词（Antivirus、Endpoint Protection 等）
fn is_security_app(app: &WindowsAppInfo) -> bool {
    // 规则 1: 厂商关键词（匹配 name 或 publisher）
    for &vendor in SECURITY_VENDOR_KEYWORDS {
        let vendor_lower = vendor.to_lowercase();
        if app.name.to_lowercase().contains(&vendor_lower) {
            return true;
        }
        if let Some(ref pub_name) = app.publisher {
            if pub_name.to_lowercase().contains(&vendor_lower) {
                return true;
            }
        }
    }

    // 规则 2: 安全类别关键词（仅匹配 name）
    let name_lower = app.name.to_lowercase();
    if SECURITY_CATEGORY_KEYWORDS
        .iter()
        .any(|kw| name_lower.contains(kw))
    {
        return true;
    }

    false
}

/// 判断是否为数据保护应用
///
/// 检测规则（任一命中即视为数据保护）：
/// 1. 应用名包含功能类别关键词（password、输入法、rime 等）
/// 2. 数据目录大小启发式：应用在 %APPDATA% 或 %LOCALAPPDATA% 下的数据目录
///    超过 50MB，视为含有用户重要数据（聊天记录、文档等）
fn is_data_protected_app(app: &WindowsAppInfo) -> bool {
    // 规则 1: 功能类别关键词
    let name_lower = app.name.to_lowercase();
    if DATA_PROTECTED_KEYWORDS
        .iter()
        .any(|kw| name_lower.contains(kw))
    {
        return true;
    }

    // 规则 2: 数据目录大小启发式
    // 检查 %APPDATA%/<appname> 和 %LOCALAPPDATA%/<appname> 是否存在且 >50MB
    // 这使得 IM 应用（微信、QQ、Telegram 等）无需枚举即可被保护
    if has_large_user_data(&app.name) {
        return true;
    }

    false
}

/// 检查应用是否有大量用户数据（>50MB）
///
/// 在 %APPDATA% 和 %LOCALAPPDATA% 下查找与应用名匹配的目录，
/// 如果目录大小超过阈值，返回 true。
/// 这是一种通用启发式，可捕获所有 IM/云盘/邮件应用而无需枚举。
fn has_large_user_data(app_name: &str) -> bool {
    let home = home_dir();
    let local_appdata = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local"));
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Roaming"));

    // 保护性启发式：命中就"不让卸载"，方向和删除相反，所以不走
    // `is_residual_dir` 那套严格规则 —— 那边漏报是安全，这边漏报是少了一层
    // 保护（fail-open）。用宽松近似的代价只是"这个应用本工具不帮你卸"，并且后面还有 50MB 体积阈值兜底。
    let trimmed_name = app_name
        .trim_end_matches(" for windows")
        .trim_end_matches(" desktop")
        .trim_end_matches(" beta")
        .trim_end_matches(" (64-bit)");

    for base in [&local_appdata, &appdata] {
        if !base.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                if resembles_app_dir(&dir_name, trimmed_name) {
                    let path = entry.path();
                    if path.is_dir() {
                        let size = dir_size(&path);
                        if size >= DATA_PROTECTED_SIZE_THRESHOLD {
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

// =========================================================================
//  注册表扫描
// =========================================================================

/// 查询注册表 Uninstall 键下所有应用
///
/// 使用 `reg query` 命令查询三个注册表位置。
fn scan_registry_uninstall() -> Vec<WindowsAppInfo> {
    let mut apps = Vec::new();

    let reg_paths = [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    for reg_path in &reg_paths {
        // 列出所有子键
        let output = Command::new("reg").args(["query", reg_path]).output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let line = line.trim();
                if !line.starts_with("HKEY") {
                    continue;
                }
                // 提取键名（最后一段）
                let key_name = line.rsplit('\\').next().unwrap_or("").to_string();
                if key_name.is_empty() {
                    continue;
                }
                // 查询该键的值
                if let Some(app) = query_app_info(reg_path, &key_name) {
                    apps.push(app);
                }
            }
        }
    }

    apps
}

/// 查询单个注册表键的应用信息
fn query_app_info(parent_path: &str, key_name: &str) -> Option<WindowsAppInfo> {
    let full_path = format!("{}\\{}", parent_path, key_name);
    let output = Command::new("reg").args(["query", &full_path]).output();
    let out = output.ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut name = None;
    let mut install_location = None;
    let mut uninstall_string = None;
    let mut quiet_uninstall_string = None;
    let mut estimated_size = None;
    let mut publisher = None;
    let mut system_component = false;

    for line in stdout.lines() {
        let line = line.trim();
        // reg query 输出格式: "ValueName    REG_SZ    ValueData"
        if let Some(val) = parse_reg_line(line, "DisplayName") {
            name = Some(val);
        } else if let Some(val) = parse_reg_line(line, "InstallLocation") {
            if !val.is_empty() {
                install_location = Some(val);
            }
        } else if let Some(val) = parse_reg_line(line, "UninstallString") {
            uninstall_string = Some(val);
        } else if let Some(val) = parse_reg_line(line, "QuietUninstallString") {
            quiet_uninstall_string = Some(val);
        } else if let Some(val) = parse_reg_line(line, "EstimatedSize") {
            estimated_size = val.parse::<u64>().ok();
        } else if let Some(val) = parse_reg_line(line, "Publisher") {
            publisher = Some(val);
        } else if let Some(val) = parse_reg_line(line, "SystemComponent") {
            // SystemComponent = 1 表示系统组件（Windows 自带的隐藏标记）
            system_component = val.trim() == "1";
        }
    }

    let name = name?;
    // 跳过没有名称或名称为空的应用
    if name.trim().is_empty() {
        return None;
    }

    Some(WindowsAppInfo {
        name,
        install_location,
        uninstall_string,
        quiet_uninstall_string,
        estimated_size, // KB
        publisher,
        key_name: key_name.to_string(),
        is_uwp: false,
        system_component,
    })
}

/// 解析 reg query 输出行
///
/// 格式: "    DisplayName    REG_SZ    My Application"
/// 或:   "    EstimatedSize    REG_DWORD    0x1234"
fn parse_reg_line(line: &str, field: &str) -> Option<String> {
    if !line.to_lowercase().contains(&field.to_lowercase()) {
        return None;
    }
    // 分割: 字段名 | 类型 | 值
    let parts: Vec<&str> = line.splitn(3, "    ").collect();
    if parts.len() < 3 {
        // 尝试用多个空格分割
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            return None;
        }
        // 找到 REG_ 类型后的内容
        let type_idx = parts.iter().position(|p| p.starts_with("REG_"))?;
        let value = parts[type_idx + 1..].join(" ");
        // REG_DWORD 的值是 0x 开头的十六进制
        if parts[type_idx].contains("DWORD") {
            return Some(value);
        }
        return Some(value);
    }
    let value = parts[2].trim().to_string();
    if value.is_empty() {
        return None;
    }
    // REG_DWORD: 0x1234 -> 转十进制
    if parts[1].contains("DWORD") && value.starts_with("0x") {
        if let Ok(n) = u64::from_str_radix(&value[2..], 16) {
            return Some(n.to_string());
        }
    }
    Some(value)
}

/// 扫描 UWP/Store 应用（通过 PowerShell）
fn scan_uwp_apps() -> Vec<WindowsAppInfo> {
    let ps_cmd =
        "Get-AppxPackage | Select-Object Name, PackageFullName, InstallLocation | ConvertTo-Json";
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_cmd])
        .output();
    let mut apps = Vec::new();
    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        // 解析 JSON（简化处理，PowerShell 可能返回数组或单对象）
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
            let arr = if json.is_array() {
                json.as_array().unwrap().clone()
            } else {
                vec![json]
            };
            for item in arr {
                let name = item
                    .get("Name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let full_name = item
                    .get("PackageFullName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let install = item
                    .get("InstallLocation")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if name.is_empty() {
                    continue;
                }
                apps.push(WindowsAppInfo {
                    name,
                    install_location: install,
                    uninstall_string: Some(format!("Remove-AppxPackage -Package {}", full_name)),
                    quiet_uninstall_string: Some(format!(
                        "Remove-AppxPackage -Package {}",
                        full_name
                    )),
                    estimated_size: None,
                    publisher: None,
                    key_name: full_name,
                    is_uwp: true,
                    system_component: false,
                });
            }
        }
    }
    apps
}

/// 直接扫描 C:\Program Files\ 和 C:\Program Files (x86)\ 目录
///
/// 查找包含卸载程序（uninstall.exe、unins000.exe 等）的应用目录，
/// 补充注册表中未登记的应用（便携应用、手动解压安装的工具等）。
///
/// 默认只扫描 C 盘，开启 set_scan_all_disks(true) 后扫描所有盘符。
fn scan_program_files_dirs() -> Vec<WindowsAppInfo> {
    let mut apps = Vec::new();

    // 卸载程序文件名（按常见程度排序）
    let uninstaller_names = [
        "uninstall.exe",
        "uninst.exe",
        "unins000.exe",
        "unins001.exe",
        "uninstaller.exe",
        "uninstall-helper.exe",
    ];

    // 搜索目录：默认 C:\Program Files\、C:\Program Files (x86)\
    let mut search_dirs: Vec<PathBuf> = vec![
        PathBuf::from(r"C:\Program Files"),
        PathBuf::from(r"C:\Program Files (x86)"),
    ];

    // 开启全盘扫描后，检测其他盘符（D:、E: 等）的 Program Files 目录
    if should_scan_all_disks() {
        crate::logger::info("扫描全部磁盘的 Program Files 目录");
        for drive in ['D', 'E', 'F', 'G', 'H', 'I', 'J'] {
            let pf = PathBuf::from(format!("{}:\\Program Files", drive));
            let pf_x86 = PathBuf::from(format!("{}:\\Program Files (x86)", drive));
            if pf.is_dir() {
                search_dirs.push(pf);
            }
            if pf_x86.is_dir() {
                search_dirs.push(pf_x86);
            }
        }
    }

    for base in &search_dirs {
        let entries = match std::fs::read_dir(base) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = entry.file_name().to_string_lossy().to_string();

            // 跳过系统目录
            let dir_lower = dir_name.to_lowercase();
            if dir_lower.starts_with("windows")
                || dir_lower.starts_with("microsoft")
                || dir_lower.starts_with("common files")
                || dir_lower == "internet explorer"
                || dir_lower == "windowsapps"
            {
                continue;
            }

            // 在应用目录中查找卸载程序
            for uninst_name in &uninstaller_names {
                let uninst_path = path.join(uninst_name);
                if uninst_path.exists() {
                    let size = dir_size(&path);
                    apps.push(WindowsAppInfo {
                        name: dir_name.clone(),
                        install_location: Some(path.to_string_lossy().to_string()),
                        uninstall_string: Some(uninst_path.to_string_lossy().to_string()),
                        quiet_uninstall_string: None,
                        estimated_size: Some(size / 1024),
                        publisher: None,
                        key_name: format!("dir:{}", dir_name),
                        is_uwp: false,
                        system_component: false,
                    });
                    break; // 每个目录只添加一个卸载入口
                }
            }
        }
    }

    apps
}

/// 获取所有已安装应用（注册表 + UWP + Program Files 直接扫描）
///
/// 三个数据源并行扫描，加速应用列表获取。
pub fn get_installed_apps() -> Vec<WindowsAppInfo> {
    crate::logger::info("应用列表并行扫描开始（注册表 + UWP + Program Files）");

    let (mut apps, uwp_apps, dir_apps) = std::thread::scope(|s| {
        let h_reg = s.spawn(scan_registry_uninstall);
        let h_uwp = s.spawn(scan_uwp_apps);
        let h_dir = s.spawn(scan_program_files_dirs);
        (
            h_reg.join().unwrap_or_default(),
            h_uwp.join().unwrap_or_default(),
            h_dir.join().unwrap_or_default(),
        )
    });

    apps.extend(uwp_apps);

    // 合并 Program Files 直接扫描结果（去重）
    for app in dir_apps {
        let already_exists = apps.iter().any(|a| {
            a.name.eq_ignore_ascii_case(&app.name)
                || a.install_location.as_deref() == app.install_location.as_deref()
        });
        if !already_exists {
            apps.push(app);
        }
    }

    // 去重（按名称）
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));

    crate::logger::info(&format!("应用列表扫描完成: {} 个应用", apps.len()));
    apps
}

/// 通过注册表键名查找应用信息
///
/// 用于卸载时获取应用名称和安装路径，以支持干净卸载（注册表 + 环境变量清理）。
pub fn find_app_by_key(key_name: &str) -> Option<WindowsAppInfo> {
    let apps = get_installed_apps();
    apps.into_iter()
        .find(|a| a.key_name.eq_ignore_ascii_case(key_name))
}

// =========================================================================
//  已安装应用名称收集
// =========================================================================

/// 收集所有已安装应用的名称变体集合
///
/// 基于 `get_installed_apps()` 返回的应用列表，生成小写、去空格/连字符/下划线
/// 以及去除常见后缀的变体，用于和 AppData 目录名做模糊匹配。
fn get_installed_app_names() -> HashSet<String> {
    let mut names = HashSet::new();

    for app in get_installed_apps() {
        let lower = app.name.to_lowercase();
        if lower.is_empty() {
            continue;
        }

        // 原始小写名
        names.insert(lower.clone());

        // 去除空格/连字符/下划线后的紧凑名
        let compact = lower.replace([' ', '-', '_'], "");
        if !compact.is_empty() && compact != lower {
            names.insert(compact);
        }

        // 去除常见后缀后的名称
        for suffix in [
            " for windows",
            " desktop",
            " (64-bit)",
            " (32-bit)",
            " beta",
        ] {
            if let Some(stripped) = lower.strip_suffix(suffix) {
                names.insert(stripped.to_string());
                let compact_stripped = stripped.replace([' ', '-', '_'], "");
                if !compact_stripped.is_empty() && compact_stripped != stripped {
                    names.insert(compact_stripped);
                }
            }
        }
    }

    names
}

/// 判断 AppData 下的目录是否对应某个已安装应用
///
/// 支持模糊匹配：
/// - 目录名与已安装应用名（小写）完全一致
/// - 目录名包含已安装应用名，或应用名包含目录名
/// - 去除空格/连字符/下划线后的紧凑名互相包含
fn is_windows_app_installed(dir_name: &str, installed_names: &HashSet<String>) -> bool {
    let dir_lower = dir_name.to_lowercase();
    if installed_names.contains(&dir_lower) {
        return true;
    }

    let dir_compact = dir_lower.replace([' ', '-', '_'], "");
    if installed_names.contains(&dir_compact) {
        return true;
    }

    for name in installed_names {
        if name.len() < 3 {
            continue;
        }

        // 原始小写名的互相包含
        if dir_lower.contains(name) || name.contains(&dir_lower) {
            return true;
        }

        // 紧凑名的互相包含
        let name_compact = name.replace([' ', '-', '_'], "");
        if name_compact.len() >= 3
            && (dir_compact.contains(&name_compact) || name_compact.contains(&dir_compact))
        {
            return true;
        }
    }

    false
}

// =========================================================================
//  Windows App 残留扫描
// =========================================================================

/// 扫描 %LOCALAPPDATA% 和 %APPDATA% 下已卸载应用的残留目录
///
/// 对每个一级目录：
/// - 跳过 `is_windows_system_name` 匹配的系统目录
/// - 跳过与已安装应用名匹配的目录
/// - 剩余目录作为独立的 `App残留` 项加入扫描结果
///
/// 对 `Programs` 目录特殊处理：遍历其子目录，逐个判断是否对应已安装应用。
fn scan_windows_app_leftovers(items: &mut Vec<ScanItem>) {
    let home = home_dir();
    let local_appdata = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local"));
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Roaming"));

    let installed_names = get_installed_app_names();

    for base in [&local_appdata, &appdata] {
        if !base.is_dir() {
            continue;
        }

        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = entry.file_name().to_string_lossy().to_string();

            // 跳过系统级目录
            if is_windows_system_name(&dir_name) {
                continue;
            }

            // 对 Programs 目录做特殊处理：扫描子目录
            if dir_name.eq_ignore_ascii_case("Programs") {
                scan_programs_subdirs(&path, &installed_names, items);
                continue;
            }

            // 跳过已安装应用对应的目录
            if is_windows_app_installed(&dir_name, &installed_names) {
                continue;
            }

            let size = dir_size(&path);
            if size == 0 {
                continue;
            }

            items.push(ScanItem {
                path: path.to_string_lossy().to_string(),
                size_bytes: size,
                category: "App残留".to_string(),
                selected: false,
                deletable: true,
                undeletable_reason: String::new(),
                batch_paths: Vec::new(),
                recommend: Recommend::Caution,
                description: format!("{} 可能为已卸载应用的残留目录，删除前请确认", dir_name),
            });
        }
    }
}

/// 扫描 Programs 目录下的子目录
///
/// 每个子目录单独判断是否对应已安装应用；未匹配到的作为 App残留 项。
fn scan_programs_subdirs(
    programs_dir: &Path,
    installed_names: &HashSet<String>,
    items: &mut Vec<ScanItem>,
) {
    let Ok(entries) = std::fs::read_dir(programs_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();

        if is_windows_system_name(&dir_name) {
            continue;
        }

        if is_windows_app_installed(&dir_name, installed_names) {
            continue;
        }

        let size = dir_size(&path);
        if size == 0 {
            continue;
        }

        items.push(ScanItem {
            path: path.to_string_lossy().to_string(),
            size_bytes: size,
            category: "App残留".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: Recommend::Caution,
            description: format!(
                "{} 可能为已卸载应用的残留目录（位于 Programs），删除前请确认",
                dir_name
            ),
        });
    }
}

// =========================================================================
//  App 缓存扫描器
// =========================================================================

/// Windows App 缓存扫描器
///
/// 扫描 %LOCALAPPDATA% 和 %APPDATA% 下的应用 Cache 目录
pub struct WindowsAppCacheScanner;

impl WindowsAppCacheScanner {
    pub fn new() -> Self {
        Self
    }
}

/// 缓存目录名模式（%LOCALAPPDATA% / %APPDATA% 下）
const CACHE_DIR_NAMES: &[&str] = &[
    "Cache",
    "cache",
    "Caches",
    "caches",
    "GPUCache",
    "Code Cache",
    "Service Worker",
];

/// Program Files 下的日志/缓存目录名模式
const PF_CACHE_NAMES: &[&str] = &["logs", "log", "temp", "tmp", "Cache", "cache"];

/// 扫描用户 AppData 目录下的缓存（%LOCALAPPDATA% 或 %APPDATA%）
///
/// 遍历 base 下的每个应用目录，查找名为 Cache/GPUCache/Code Cache 等的子目录。
fn scan_user_appdata_cache(base: &Path) -> Vec<ScanItem> {
    let mut items = Vec::new();
    if !base.is_dir() {
        return items;
    }
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return items,
    };

    for entry in entries.flatten() {
        let app_dir = entry.path();
        if !app_dir.is_dir() {
            continue;
        }
        let app_name = entry.file_name().to_string_lossy().to_string();

        if let Ok(sub_entries) = std::fs::read_dir(&app_dir) {
            for sub in sub_entries.flatten() {
                let sub_name = sub.file_name().to_string_lossy().to_string();
                if CACHE_DIR_NAMES
                    .iter()
                    .any(|&cn| sub_name.eq_ignore_ascii_case(cn))
                {
                    let cache_path = sub.path();
                    let size = dir_size(&cache_path);
                    if size > 0 {
                        items.push(ScanItem {
                            path: cache_path.to_string_lossy().to_string(),
                            size_bytes: size,
                            category: format!("{} 缓存", app_name),
                            selected: false,
                            deletable: true,
                            undeletable_reason: String::new(),
                            batch_paths: Vec::new(),
                            recommend: Recommend::CacheOnly,
                            description: format!(
                                "{} 应用的缓存文件，删除后不影响使用，应用会自动重建",
                                app_name
                            ),
                        });
                    }
                }
            }
        }
    }
    items
}

/// 扫描 Program Files 目录下的日志/缓存子目录
///
/// 跳过 Windows/Microsoft 系统目录，只显示 >1MB 的目录。
/// Program Files 下的目录需要管理员权限删除，标记为不可删除。
fn scan_program_files_cache_dir(base: &Path) -> Vec<ScanItem> {
    let mut items = Vec::new();
    if !base.is_dir() {
        return items;
    }
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return items,
    };

    for app_entry in entries.flatten() {
        let app_dir = app_entry.path();
        if !app_dir.is_dir() {
            continue;
        }
        let app_name = app_entry.file_name().to_string_lossy().to_string();

        // 跳过系统目录
        let dir_lower = app_name.to_lowercase();
        if dir_lower.starts_with("windows") || dir_lower.starts_with("microsoft") {
            continue;
        }

        if let Ok(sub_entries) = std::fs::read_dir(&app_dir) {
            for sub in sub_entries.flatten() {
                let sub_name = sub.file_name().to_string_lossy().to_string();
                if PF_CACHE_NAMES
                    .iter()
                    .any(|&cn| sub_name.eq_ignore_ascii_case(cn))
                {
                    let cache_path = sub.path();
                    let size = dir_size(&cache_path);
                    if size > 1024 * 1024 {
                        // 只显示 >1MB 的目录
                        items.push(ScanItem {
                            path: cache_path.to_string_lossy().to_string(),
                            size_bytes: size,
                            category: format!("{} 缓存", app_name),
                            selected: false,
                            deletable: false, // Program Files 需要管理员权限
                            undeletable_reason: "需要管理员权限删除".to_string(),
                            batch_paths: Vec::new(),
                            recommend: Recommend::CacheOnly,
                            description: format!(
                                "{} 应用的日志/缓存目录（位于 Program Files，需管理员权限）",
                                app_name
                            ),
                        });
                    }
                }
            }
        }
    }
    items
}

/// 收集需要扫描的 Program Files 目录列表（尊重磁盘扫描配置）
fn collect_program_files_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from(r"C:\Program Files"),
        PathBuf::from(r"C:\Program Files (x86)"),
    ];
    if should_scan_all_disks() {
        for drive in ['D', 'E', 'F', 'G', 'H', 'I', 'J'] {
            let pf = PathBuf::from(format!("{}:\\Program Files", drive));
            let pf_x86 = PathBuf::from(format!("{}:\\Program Files (x86)", drive));
            if pf.is_dir() {
                dirs.push(pf);
            }
            if pf_x86.is_dir() {
                dirs.push(pf_x86);
            }
        }
    }
    dirs
}

impl Scanner for WindowsAppCacheScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let home = home_dir();

        let local_appdata = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Local"));
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Roaming"));

        let pf_dirs = collect_program_files_dirs();

        crate::logger::info(&format!(
            "App缓存并行扫描开始: AppData 目录 2 个, Program Files 目录 {} 个 (磁盘配置: {})",
            pf_dirs.len(),
            if should_scan_all_disks() {
                "全部磁盘"
            } else {
                "仅 C 盘"
            }
        ));

        // 并行扫描：%LOCALAPPDATA%、%APPDATA%、Program Files 各一个线程
        // 使用 std::thread::scope 确保线程安全，无需 Arc/Mutex
        let (local_items, roaming_items, pf_items) = std::thread::scope(|s| {
            let h_local = s.spawn(|| scan_user_appdata_cache(&local_appdata));
            let h_roaming = s.spawn(|| scan_user_appdata_cache(&appdata));

            // Program Files 多目录顺序扫描（单线程，因为通常只有 2 个目录）
            let h_pf = s.spawn(|| {
                let mut all_pf_items = Vec::new();
                for dir in &pf_dirs {
                    let part = scan_program_files_cache_dir(dir);
                    all_pf_items.extend(part);
                }
                all_pf_items
            });

            (
                h_local.join().unwrap_or_default(),
                h_roaming.join().unwrap_or_default(),
                h_pf.join().unwrap_or_default(),
            )
        });

        let mut items = local_items;
        items.extend(roaming_items);
        items.extend(pf_items);

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        crate::logger::info(&format!(
            "App缓存扫描完成: {} 项, {}, 耗时 {}ms",
            items.len(),
            super::format_size(total_size),
            start.elapsed().as_millis()
        ));

        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

// =========================================================================
//  App 数据扫描器
// =========================================================================

/// Windows App 数据扫描器
///
/// 扫描 %LOCALAPPDATA% 和 %APPDATA% 下的应用数据目录
pub struct WindowsAppDataScanner;

impl WindowsAppDataScanner {
    pub fn new() -> Self {
        Self
    }
}

/// App 数据扫描时跳过的系统目录
const APPDATA_SKIP_DIRS: &[&str] = &[
    "Microsoft",
    "Packages",
    "ConnectedDevicesPlatform",
    "Temp",
    "CrashDumps",
    "D3DSCache",
    "DXCache",
    "NVIDIA",
    "AMD",
    "IconCache.db",
    "cache",
    "Cache",
];

/// 扫描 AppData 目录下的应用数据（%LOCALAPPDATA% 或 %APPDATA%）
fn scan_user_appdata_data(base: &Path) -> Vec<ScanItem> {
    let mut items = Vec::new();
    if !base.is_dir() {
        return items;
    }
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return items,
    };

    for entry in entries.flatten() {
        let app_dir = entry.path();
        if !app_dir.is_dir() {
            continue;
        }
        let app_name = entry.file_name().to_string_lossy().to_string();

        // 跳过系统目录
        if APPDATA_SKIP_DIRS
            .iter()
            .any(|&s| app_name.eq_ignore_ascii_case(s))
        {
            continue;
        }

        let size = dir_size(&app_dir);
        if size > 10 * 1024 * 1024 {
            // > 10MB
            items.push(ScanItem {
                path: app_dir.to_string_lossy().to_string(),
                size_bytes: size,
                category: format!("{} 数据", app_name),
                selected: false,
                deletable: false, // App 数据默认不可删除（高风险）
                undeletable_reason: "应用数据删除可能导致应用配置丢失，请确认后手动删除"
                    .to_string(),
                batch_paths: Vec::new(),
                recommend: Recommend::Advanced,
                description: format!(
                    "{} 应用的本地数据目录。\n⚠️ 删除可能导致应用配置、登录状态丢失，建议先备份。",
                    app_name
                ),
            });
        }
    }
    items
}

impl Scanner for WindowsAppDataScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        let home = home_dir();

        let local_appdata = std::env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Local"));
        let appdata = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join("AppData/Roaming"));

        crate::logger::info("App数据并行扫描开始");

        // 并行扫描 %LOCALAPPDATA% 和 %APPDATA%
        let (local_items, roaming_items) = std::thread::scope(|s| {
            let h_local = s.spawn(|| scan_user_appdata_data(&local_appdata));
            let h_roaming = s.spawn(|| scan_user_appdata_data(&appdata));
            (
                h_local.join().unwrap_or_default(),
                h_roaming.join().unwrap_or_default(),
            )
        });

        let mut items = local_items;
        items.extend(roaming_items);

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        crate::logger::info(&format!(
            "App数据扫描完成: {} 项, {}, 耗时 {}ms",
            items.len(),
            super::format_size(total_size),
            start.elapsed().as_millis()
        ));
        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

// =========================================================================
//  App 卸载扫描器
// =========================================================================

/// Windows App 卸载扫描器
///
/// 扫描注册表中的已安装应用，支持卸载操作
pub struct WindowsUninstallScanner;

impl WindowsUninstallScanner {
    pub fn new() -> Self {
        Self
    }
}

impl Scanner for WindowsUninstallScanner {
    fn scan(&self) -> ScanResult {
        let start = std::time::Instant::now();
        crate::logger::info("Windows 卸载扫描开始");
        let apps = get_installed_apps();
        crate::logger::info(&format!(
            "发现 {} 个已安装应用（注册表 + UWP + Program Files 直接扫描）",
            apps.len()
        ));
        let mut items = Vec::new();

        for app in apps {
            let protection = check_protection(&app);
            let bloatware_match = find_bloatware_match(&app);

            // 记录保护检测日志
            match protection {
                WinProtectionLevel::Critical => {
                    crate::logger::info(&format!("保护检测: {} -> Critical (系统关键)", app.name));
                }
                WinProtectionLevel::RequiresOfficialUninstaller => {
                    crate::logger::info(&format!(
                        "保护检测: {} -> RequiresOfficialUninstaller",
                        app.name
                    ));
                }
                WinProtectionLevel::DataProtected => {
                    crate::logger::info(&format!(
                        "保护检测: {} -> DataProtected (数据保护)",
                        app.name
                    ));
                }
                WinProtectionLevel::None => {}
            }

            // 记录 bloatware 匹配日志
            if let Some(ref bw) = bloatware_match {
                crate::logger::info(&format!(
                    "Bloatware 匹配: {} -> {} ({})",
                    app.name,
                    bw.friendly_name,
                    match bw.removal_method {
                        RemovalMethod::Appx => "Appx",
                        RemovalMethod::WinGet => "WinGet",
                        RemovalMethod::AppxOrWinGet => "AppxOrWinGet",
                    }
                ));
            }

            let (deletable, reason, desc_suffix) = match protection {
                WinProtectionLevel::Critical => {
                    (false, "系统关键应用，禁止卸载".to_string(), " [系统保护]")
                }
                WinProtectionLevel::RequiresOfficialUninstaller => (
                    false,
                    "安全软件，请使用官方卸载工具".to_string(),
                    " [需官方卸载工具]",
                ),
                WinProtectionLevel::DataProtected => (true, String::new(), " [⚠️ 数据保护]"),
                WinProtectionLevel::None => (true, String::new(), ""),
            };

            // 计算大小
            let size = if let Some(ref loc) = app.install_location {
                let p = Path::new(loc);
                if p.is_dir() {
                    dir_size(p)
                } else {
                    app.estimated_size.unwrap_or(0) * 1024
                }
            } else {
                app.estimated_size.unwrap_or(0) * 1024
            };

            // 卸载路径标记（删除时识别）
            let uninstall_path = if app.is_uwp {
                format!("uwp:{}", app.key_name)
            } else if app.uninstall_string.is_some() {
                format!("uninstall:{}", app.key_name)
            } else {
                continue; // 没有卸载命令的应用跳过
            };

            let display_size = if size > 0 {
                super::format_size(size)
            } else {
                "未知".to_string()
            };

            // 可卸载应用：拆为三项，让用户自主选择
            //   1. 应用卸载（Caution）：只运行卸载程序/删除安装目录
            //   2. 应用数据（Advanced）：删 AppData 中非缓存的数据/配置
            //   3. 应用缓存（CacheOnly）：删 AppData 中 Cache 类子目录
            let (data_paths, data_size, cache_paths, cache_size) = if deletable {
                scan_windows_app_data_and_cache(&app.name)
            } else {
                (Vec::new(), 0, Vec::new(), 0)
            };

            // 确定卸载项的推荐等级和描述
            // Bloatware 匹配优先，其次保护级别，默认 Caution
            let (uninstall_recommend, uninstall_desc) = if let Some(ref bw) = bloatware_match {
                let desc = format!(
                    "🗑️ {} ({}) - {} | {} | 大小: {}",
                    app.name,
                    if app.is_uwp { "UWP" } else { "Win32" },
                    bw.friendly_name,
                    bw.description,
                    display_size
                );
                (bw.recommend, desc)
            } else {
                let desc = format!(
                    "{} ({}){} - 大小: {} | 仅卸载应用本体，不会删除用户数据与缓存",
                    app.name,
                    if app.is_uwp { "UWP" } else { "Win32" },
                    desc_suffix,
                    display_size
                );
                (Recommend::Caution, desc)
            };

            // 1) 应用卸载项：只卸载应用本体
            items.push(ScanItem {
                path: uninstall_path,
                size_bytes: size,
                category: format!("{} (卸载)", app.name),
                selected: bloatware_match
                    .as_ref()
                    .map(|bw| bw.selected_by_default)
                    .unwrap_or(false),
                deletable,
                undeletable_reason: reason,
                batch_paths: Vec::new(),
                recommend: uninstall_recommend,
                description: uninstall_desc,
            });

            // 2) 应用数据项（Advanced）：删数据类文件
            if deletable && data_size > 0 && !data_paths.is_empty() {
                let data_desc = if matches!(protection, WinProtectionLevel::DataProtected) {
                    format!(
                        "{} 的应用数据（含聊天记录/配置等），删除前请务必备份",
                        app.name
                    )
                } else {
                    format!(
                        "{} 的应用数据与配置（{} 项），删除后可能需要重新登录或配置",
                        app.name,
                        data_paths.len()
                    )
                };
                items.push(ScanItem {
                    path: data_paths[0].clone(),
                    size_bytes: data_size,
                    category: format!("{} 数据", app.name),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: data_paths.clone(),
                    recommend: Recommend::Advanced,
                    description: data_desc,
                });
            }

            // 3) 缓存清理项（CacheOnly）：Cache / GPUCache / Code Cache 等
            if deletable && cache_size > 0 && !cache_paths.is_empty() {
                items.push(ScanItem {
                    path: cache_paths[0].clone(),
                    size_bytes: cache_size,
                    category: format!("{} 缓存", app.name),
                    selected: false,
                    deletable: true,
                    undeletable_reason: String::new(),
                    batch_paths: cache_paths.clone(),
                    recommend: Recommend::CacheOnly,
                    description: format!(
                        "{} 的缓存/日志文件，删除后不影响使用，应用会自动重建",
                        app.name
                    ),
                });
            }

            if deletable && (!data_paths.is_empty() || !cache_paths.is_empty()) {
                crate::logger::info(&format!(
                    "{} 拆分完成: 数据 {} 项 ({}), 缓存 {} 项 ({})",
                    app.name,
                    data_paths.len(),
                    super::format_size(data_size),
                    cache_paths.len(),
                    super::format_size(cache_size)
                ));
            }
        }

        // 扫描已卸载应用留下的 AppData 目录残留
        scan_windows_app_leftovers(&mut items);
        crate::logger::info(&format!(
            "Windows App残留扫描完成，当前共 {} 项",
            items.len()
        ));

        let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
        ScanResult {
            items,
            total_size,
            scan_time_ms: start.elapsed().as_millis() as u64,
        }
    }
}

/// 根据应用名扫描其数据目录和缓存目录
///
/// 在 %LOCALAPPDATA% 和 %APPDATA% 下查找与应用名匹配的目录，
/// 返回 (数据路径列表, 数据总大小, 缓存路径列表, 缓存总大小)。
/// 缓存子目录按 CACHE_DIR_NAMES 匹配，其余子目录/根文件视为数据。
fn scan_windows_app_data_and_cache(app_name: &str) -> (Vec<String>, u64, Vec<String>, u64) {
    let home = home_dir();
    let local_appdata = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local"));
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Roaming"));

    let mut data_paths = Vec::new();
    let mut data_size = 0u64;
    let mut cache_paths = Vec::new();
    let mut cache_size = 0u64;

    let name_variants = windows_app_name_variants(app_name);

    for base in [&local_appdata, &appdata] {
        if !base.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(base) else {
            continue;
        };

        for entry in entries.flatten() {
            let app_dir = entry.path();
            if !app_dir.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            let dir_lower = dir_name.to_lowercase();

            // 检查目录名是否匹配应用名变体
            if !name_variants
                .iter()
                .any(|v| dir_lower.contains(&v.to_lowercase()))
            {
                continue;
            }

            let Ok(subs) = std::fs::read_dir(&app_dir) else {
                continue;
            };

            for sub in subs.flatten() {
                let sub_path = sub.path();
                let sub_name = sub.file_name().to_string_lossy().to_string();

                if sub_path.is_dir()
                    && CACHE_DIR_NAMES
                        .iter()
                        .any(|&cn| sub_name.eq_ignore_ascii_case(cn))
                {
                    // 缓存子目录
                    let size = dir_size(&sub_path);
                    if size > 0 {
                        cache_paths.push(sub_path.to_string_lossy().to_string());
                        cache_size += size;
                    }
                } else {
                    // 数据文件或数据子目录
                    let size = if sub_path.is_dir() {
                        dir_size(&sub_path)
                    } else {
                        sub.metadata().map(|m| m.len()).unwrap_or(0)
                    };
                    if size > 0 {
                        data_paths.push(sub_path.to_string_lossy().to_string());
                        data_size += size;
                    }
                }
            }
        }
    }

    (data_paths, data_size, cache_paths, cache_size)
}

/// 生成 Windows 应用名变体（用于匹配 AppData 目录）
fn windows_app_name_variants(app_name: &str) -> Vec<String> {
    let mut variants = vec![app_name.to_string()];

    let lower = app_name.to_lowercase();
    if lower != app_name {
        variants.push(lower.clone());
    }

    // 去除空格、连字符、下划线后的紧凑名
    let compact = app_name.replace([' ', '-', '_'], "").to_lowercase();
    if compact != lower && !compact.is_empty() {
        variants.push(compact);
    }

    // 去除常见后缀
    for suffix in [
        " for windows",
        " desktop",
        " (64-bit)",
        " (32-bit)",
        " beta",
    ] {
        if lower.ends_with(suffix) {
            let stripped = lower.strip_suffix(suffix).unwrap_or(&lower).to_string();
            if !variants.contains(&stripped) {
                variants.push(stripped);
            }
        }
    }

    variants
}

// =========================================================================
//  应用卸载执行
// =========================================================================

/// 卸载 Windows 应用
///
/// 根据 path 前缀决定卸载方式：
/// - `uwp:<PackageFullName>` -> PowerShell Remove-AppxPackage
/// - `uninstall:<KeyName>` -> 查注册表执行 UninstallString
///
/// 返回 (success, output_message)
pub fn uninstall_app(path: &str) -> (bool, String) {
    if path.starts_with("uwp:") {
        let package_full_name = &path[4..];
        uninstall_uwp(package_full_name)
    } else if path.starts_with("uninstall:") {
        let key_name = &path[10..];
        uninstall_win32(key_name)
    } else {
        (false, "未知的卸载路径格式".to_string())
    }
}

/// 卸载 UWP/Store 应用
///
/// 根据 BLOATWARE_DB 中的 removal_method 决定卸载方式：
/// - Appx（默认）:   Remove-AppxPackage，失败尝试 -AllUsers
/// - WinGet:         优先 winget uninstall，失败回退 Appx
/// - AppxOrWinGet:   优先 Appx，失败回退 WinGet
/// - 未匹配条目:     默认 Appx
///
/// Appx 卸载成功后会尝试清理 provisioned package（防止新用户登录时重装）。
fn uninstall_uwp(package_full_name: &str) -> (bool, String) {
    // PackageFullName 格式: <Name>_<Version>_<Arch>_<ResourceId>_<PublisherId>
    let package_name = package_full_name
        .split('_')
        .next()
        .unwrap_or(package_full_name);
    let entry = bloatware_db()
        .iter()
        .find(|e| e.app_id.eq_ignore_ascii_case(package_name));

    match entry.map(|e| e.removal_method) {
        Some(RemovalMethod::WinGet) => {
            let r = uninstall_uwp_winget(entry.unwrap());
            if r.0 {
                r
            } else {
                // winget 失败（可能未安装），回退 Appx
                let fallback = uninstall_uwp_appx(package_full_name);
                if fallback.0 {
                    fallback
                } else {
                    r
                }
            }
        }
        Some(RemovalMethod::AppxOrWinGet) => {
            let r = uninstall_uwp_appx(package_full_name);
            if r.0 {
                r
            } else {
                uninstall_uwp_winget(entry.unwrap())
            }
        }
        _ => uninstall_uwp_appx(package_full_name),
    }
}

/// 通过 PowerShell Remove-AppxPackage 卸载 UWP 应用
///
/// 1. 先按当前用户卸载（Remove-AppxPackage -Package）
/// 2. 失败则尝试所有用户（Get-AppxPackage -AllUsers | Remove-AppxPackage -AllUsers，需管理员）
/// 3. 成功后清理 provisioned package（阻止重装后复活，失败不影响结果）
fn uninstall_uwp_appx(package_full_name: &str) -> (bool, String) {
    let ps_cmd = format!(
        "Remove-AppxPackage -Package '{}' -ErrorAction Stop",
        package_full_name
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
        .output();

    let success = match output {
        Ok(out) => {
            if out.status.success() {
                true
            } else {
                // 当前用户卸载失败，尝试所有用户（需管理员权限）
                let package_name = package_full_name
                    .split('_')
                    .next()
                    .unwrap_or(package_full_name);
                let all_users_cmd = format!(
                    "Get-AppxPackage -AllUsers -Name '{}' | Remove-AppxPackage -AllUsers -ErrorAction Stop",
                    package_name
                );
                match Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command", &all_users_cmd])
                    .output()
                {
                    Ok(retry) if retry.status.success() => true,
                    Ok(retry) => {
                        let stderr = String::from_utf8_lossy(&retry.stderr);
                        return (
                            false,
                            format!("UWP 卸载失败（含 AllUsers 重试）: {}", stderr.trim()),
                        );
                    }
                    Err(e) => return (false, format!("执行 PowerShell 失败: {}", e)),
                }
            }
        }
        Err(e) => return (false, format!("执行 PowerShell 失败: {}", e)),
    };

    if success {
        // 清理 provisioned package（阻止新用户登录时重装，需管理员，失败不影响结果）
        let package_name = package_full_name
            .split('_')
            .next()
            .unwrap_or(package_full_name);
        cleanup_provisioned_package(package_name);
        (true, format!("UWP 应用 {} 已卸载", package_full_name))
    } else {
        (false, format!("UWP 应用 {} 卸载失败", package_full_name))
    }
}

/// 清理 provisioned package（防止应用在新用户登录时自动重装）
///
/// 需要管理员权限，失败静默忽略（仅记日志），不影响卸载主流程。
fn cleanup_provisioned_package(package_name: &str) {
    let ps_cmd = format!(
        "Get-AppxProvisionedPackage -Online | Where-Object {{$_.DisplayName -eq '{}'}} | Remove-AppxProvisionedPackage -Online -ErrorAction SilentlyContinue | Out-Null",
        package_name
    );
    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
        .output()
    {
        Ok(out) if out.status.success() => {
            crate::logger::info(&format!("已清理 provisioned package: {}", package_name));
        }
        _ => {
            crate::logger::info(&format!(
                "provisioned package 清理跳过（需管理员或不存在）: {}",
                package_name
            ));
        }
    }
}

/// 通过 winget 卸载应用
///
/// 1. 优先 `winget uninstall --id <AppId>`（精确）
/// 2. 失败则 `winget uninstall --name "<FriendlyName>"`（按名称匹配）
fn uninstall_uwp_winget(entry: &BloatwareEntryOwned) -> (bool, String) {
    // 尝试 --id 精确卸载
    let id_result = Command::new("winget")
        .args([
            "uninstall",
            "--id",
            entry.app_id.as_str(),
            "--silent",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .output();

    match id_result {
        Ok(out) if out.status.success() => {
            return (true, format!("{} 已通过 winget 卸载", entry.friendly_name));
        }
        Ok(_) => {} // 继续尝试 --name
        Err(e) => {
            // winget 命令不存在
            return (false, format!("winget 不可用: {}", e));
        }
    }

    // 回退：按名称卸载
    let name_result = Command::new("winget")
        .args([
            "uninstall",
            "--name",
            entry.friendly_name.as_str(),
            "--silent",
            "--accept-source-agreements",
            "--disable-interactivity",
        ])
        .output();

    match name_result {
        Ok(out) if out.status.success() => {
            (true, format!("{} 已通过 winget 卸载", entry.friendly_name))
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            (
                false,
                format!(
                    "winget 卸载 {} 失败: {}",
                    entry.friendly_name,
                    stderr.trim()
                ),
            )
        }
        Err(e) => (false, format!("winget 不可用: {}", e)),
    }
}

/// 卸载 Win32 应用（通过注册表 UninstallString）
fn uninstall_win32(key_name: &str) -> (bool, String) {
    // 在注册表中查找该应用
    let reg_paths = [
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    ];

    for reg_path in &reg_paths {
        let full_path = format!("{}\\{}", reg_path, key_name);
        let output = Command::new("reg").args(["query", &full_path]).output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);

            // 优先使用 QuietUninstallString（静默卸载）
            let mut quiet_cmd = None;
            let mut uninstall_cmd = None;

            for line in stdout.lines() {
                if let Some(val) = parse_reg_line(line, "QuietUninstallString") {
                    quiet_cmd = Some(val);
                } else if let Some(val) = parse_reg_line(line, "UninstallString") {
                    uninstall_cmd = Some(val);
                }
            }

            // 执行卸载命令
            if let Some(cmd) = quiet_cmd.or(uninstall_cmd) {
                return execute_uninstall_command(&cmd);
            }
        }
    }

    (false, format!("未找到应用 {} 的卸载命令", key_name))
}

/// 执行卸载命令
///
/// 解析 UninstallString 并执行，支持多种安装器类型：
/// - MSI: msiexec /x {ProductCode} -> 加 /quiet /norestart
/// - NSIS: uninstall.exe -> 加 /S
/// - Inno Setup: unins000.exe -> 加 /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
/// - InstallShield: setup.exe -> 加 /s /f1...
/// - 通用 exe: 尝试 /S，失败后用 PowerShell 提权重试
///
/// 注册表 UninstallString 包含完整路径，支持 C/D/E 任意盘符和自定义目录。
fn execute_uninstall_command(cmd_str: &str) -> (bool, String) {
    let cmd_str = cmd_str.trim();

    // MSI 安装的应用: msiexec /x{ProductCode}
    if cmd_str.to_lowercase().contains("msiexec") {
        return execute_msi_uninstall(cmd_str);
    }

    // 普通 exe 卸载程序
    let (exe_path, args) = parse_command(cmd_str);
    if exe_path.is_empty() {
        return (false, "卸载命令为空".to_string());
    }

    // 检测安装器类型，选择对应的静默参数
    let silent_flags = detect_silent_flags(&exe_path, &args);

    // 如果已有静默参数，直接使用原始参数
    let has_silent = args.iter().any(|a| {
        let lower = a.to_lowercase();
        lower == "/s"
            || lower == "/silent"
            || lower == "/quiet"
            || lower == "/verysilent"
            || lower == "--silent"
    });

    let full_args: Vec<String> = if has_silent {
        args.clone()
    } else {
        let mut combined = args.clone();
        combined.extend(silent_flags.iter().map(|s| s.to_string()));
        combined
    };

    // 尝试 1: 直接执行（可能因权限不足失败）
    crate::logger::info(&format!(
        "执行卸载命令: {} {}",
        exe_path,
        full_args.join(" ")
    ));
    let output = Command::new(&exe_path).args(&full_args).output();
    match output {
        Ok(out) if out.status.success() => {
            crate::logger::info(&format!("卸载成功: {}", exe_path));
            return (true, format!("应用 {} 已卸载", exe_path));
        }
        Ok(out) => {
            crate::logger::warn(&format!(
                "直接执行失败 (exit={}), 尝试 PowerShell 提权",
                out.status.code().unwrap_or(-1)
            ));
            // 尝试 2: 用 PowerShell Start-Process 提权执行
            let ps_result = try_powershell_elevated(&exe_path, &full_args);
            if ps_result.0 {
                return ps_result;
            }
            // 尝试 3: 原始命令（交互式，不加静默参数）
            crate::logger::info("尝试交互式卸载（不加静默参数）");
            let output = Command::new(&exe_path).args(&args).output();
            match output {
                Ok(o) if o.status.success() => (true, format!("应用 {} 已卸载", exe_path)),
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    let detail = if stderr.is_empty() {
                        format!("退出码: {}", o.status.code().unwrap_or(-1))
                    } else {
                        stderr.to_string()
                    };
                    (false, format!("卸载失败: {}", detail))
                }
                Err(e) => (false, format!("执行卸载程序失败: {}", e)),
            }
        }
        Err(e) => {
            // 直接执行失败（可能是权限不足），尝试 PowerShell 提权
            let ps_result = try_powershell_elevated(&exe_path, &full_args);
            if ps_result.0 {
                return ps_result;
            }
            (
                false,
                format!("执行卸载程序失败（直接执行和提权均失败）: {}", e),
            )
        }
    }
}

/// 执行 MSI 卸载
fn execute_msi_uninstall(cmd_str: &str) -> (bool, String) {
    if let Some(start) = cmd_str.find('{') {
        if let Some(end) = cmd_str[start..].find('}') {
            let product_code = &cmd_str[start..=start + end];
            let full_cmd = format!("msiexec /x {} /quiet /norestart", product_code);
            let parts: Vec<&str> = full_cmd.split_whitespace().collect();
            if parts.len() >= 4 {
                let output = Command::new(parts[0]).args(&parts[1..]).output();
                return match output {
                    Ok(out) if out.status.success() => {
                        (true, format!("MSI 应用 {} 已静默卸载", product_code))
                    }
                    Ok(_) => {
                        // 静默失败，尝试 /passive（显示进度条但无需交互）
                        let passive_cmd = format!("msiexec /x {} /passive", product_code);
                        let parts: Vec<&str> = passive_cmd.split_whitespace().collect();
                        let output = Command::new(parts[0]).args(&parts[1..]).output();
                        match output {
                            Ok(o) if o.status.success() => {
                                (true, format!("MSI 应用 {} 已卸载", product_code))
                            }
                            Ok(o) => (
                                false,
                                format!("MSI 卸载失败: {}", String::from_utf8_lossy(&o.stderr)),
                            ),
                            Err(e) => (false, format!("执行 msiexec 失败: {}", e)),
                        }
                    }
                    Err(e) => (false, format!("执行 msiexec 失败: {}", e)),
                };
            }
        }
    }
    (false, "MSI 卸载命令格式异常".to_string())
}

/// 检测安装器类型，返回对应的静默卸载参数
///
/// 不同安装器的静默卸载参数：
/// - NSIS: /S
/// - Inno Setup: /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
/// - InstallShield: /s /f2<logpath>（简化为 /s）
/// - WiX/MSI: 已由 msiexec 分支处理
/// - 通用: /S（NSIS 最常见）
fn detect_silent_flags(exe_path: &str, existing_args: &[String]) -> Vec<&'static str> {
    let path_lower = exe_path.to_lowercase();

    // Inno Setup: 卸载程序通常名为 unins000.exe / unins001.exe
    if path_lower.contains("unins0")
        || path_lower.contains("unins1")
        || path_lower.contains("unins2")
    {
        return vec!["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"];
    }

    // InstallShield: setup.exe 且带 -runfromtemp 或类似参数
    if (path_lower.ends_with("setup.exe") || path_lower.ends_with("_is1.exe"))
        && existing_args
            .iter()
            .any(|a| a.contains("-runfromtemp") || a.contains("-removeonly"))
    {
        return vec!["/s"];
    }

    // 默认: NSIS 风格 /S
    vec!["/S"]
}

/// 通过 PowerShell Start-Process 提权执行卸载
///
/// 使用 -Verb RunAs 触发 UAC 提权对话框，
/// -Wait 等待卸载完成。
fn try_powershell_elevated(exe_path: &str, args: &[String]) -> (bool, String) {
    let args_str = args.join(" ");
    crate::logger::info(&format!("PowerShell 提权卸载: {} {}", exe_path, args_str));

    let ps_cmd = format!(
        "Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs -Wait -PassThru | Select-Object -ExpandProperty ExitCode",
        exe_path.replace('\'', "''"),
        args_str.replace('\'', "''")
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let exit_code: i32 = stdout.parse().unwrap_or(-1);
            if out.status.success() && exit_code == 0 {
                crate::logger::info(&format!("PowerShell 提权卸载成功: {}", exe_path));
                (
                    true,
                    format!("应用 {} 已通过 PowerShell 提权卸载", exe_path),
                )
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                crate::logger::warn(&format!(
                    "PowerShell 提权卸载失败 (exit={}): {}",
                    exit_code,
                    stderr.trim()
                ));
                (
                    false,
                    format!("PowerShell 提权卸载失败 (exit={}): {}", exit_code, stderr),
                )
            }
        }
        Err(e) => (false, format!("执行 PowerShell 失败: {}", e)),
    }
}

/// 解析命令字符串为 (exe_path, args)
///
/// 处理带引号的路径：'"C:\Program Files\App\uninstall.exe" /S' -> ("C:\Program Files\App\uninstall.exe", ["/S"])
fn parse_command(cmd: &str) -> (String, Vec<String>) {
    let cmd = cmd.trim();

    if cmd.starts_with('"') {
        // 带引号的路径
        if let Some(end) = cmd[1..].find('"') {
            let exe = cmd[1..=end].to_string();
            let rest = cmd[end + 2..].trim();
            let args: Vec<String> = if rest.is_empty() {
                Vec::new()
            } else {
                rest.split_whitespace().map(String::from).collect()
            };
            return (exe, args);
        }
    }

    // 不带引号
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return (String::new(), Vec::new());
    }
    (
        parts[0].to_string(),
        parts[1..].iter().map(|s| s.to_string()).collect(),
    )
}

// =========================================================================
//  干净卸载：残留扫描 + 用户选择的清理
//
//  设计原则（用户选择权）：
//  - clean_uninstall 只运行卸载程序 + 扫描残留，不自动清理任何残留
//  - 所有残留信息返回给调用方，由 UI 呈现给用户
//  - 用户确认后才调用 clean_all_residuals 或逐项删除函数
//  - 永远不替用户做选择
//
//  残留类型：
//  1. 注册表残留（HKCU\SOFTWARE\<AppName> 等）
//  2. 环境变量残留（PATH 条目 + JAVA_HOME 等应用专属变量）
//  3. 文件系统残留（%APPDATA%\<AppName> 等）
// =========================================================================

/// 注册表残留项
#[derive(Debug, Clone)]
pub struct RegistryResidual {
    /// 注册表键路径，如 "HKCU\\SOFTWARE\\JavaSoft"
    pub key_path: String,
    /// 是否为系统级（HKLM 需要管理员权限）
    pub is_system: bool,
    /// 是否可删除（HKLM 且无管理员权限时为 false）
    pub deletable: bool,
    /// 不可删除的原因
    pub reason: String,
}

/// 环境变量残留项
#[derive(Debug, Clone)]
pub struct EnvVarResidual {
    /// 变量名，如 "JAVA_HOME" 或 "Path"
    pub var_name: String,
    /// 当前值
    pub current_value: String,
    /// 对于 Path 类型：要移除的条目列表
    /// 对于独立变量（JAVA_HOME 等）：整个值都要删除
    pub entries_to_remove: Vec<String>,
    /// 是否为系统级环境变量
    pub is_system: bool,
    /// 是否可删除
    pub deletable: bool,
    /// 不可删除的原因
    pub reason: String,
}

/// 文件系统残留项
#[derive(Debug, Clone)]
pub struct FilesystemResidual {
    /// 残留目录路径
    pub path: String,
    /// 目录大小（字节）
    pub size: u64,
    /// 是否可删除
    pub deletable: bool,
    /// 不可删除的原因
    pub reason: String,
}

/// 干净卸载残留扫描结果
#[derive(Debug, Clone)]
pub struct UninstallResidual {
    /// 注册表残留
    pub registry: Vec<RegistryResidual>,
    /// 环境变量残留
    pub env_vars: Vec<EnvVarResidual>,
    /// 文件系统残留
    pub filesystem: Vec<FilesystemResidual>,
}

impl UninstallResidual {
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty() && self.env_vars.is_empty() && self.filesystem.is_empty()
    }

    pub fn total_count(&self) -> usize {
        self.registry.len() + self.env_vars.len() + self.filesystem.len()
    }

    /// 可删除的残留项数量
    pub fn deletable_count(&self) -> usize {
        self.registry.iter().filter(|r| r.deletable).count()
            + self.env_vars.iter().filter(|e| e.deletable).count()
            + self.filesystem.iter().filter(|f| f.deletable).count()
    }
}

/// 扫描文件系统残留（只扫描，不删除）
///
/// 在 %APPDATA% 和 %LOCALAPPDATA% 下查找与应用名匹配的残留目录。
/// 所有找到的目录都标记为可删除（用户目录下无需管理员权限）。
/// 注意：这里列出的每一项在 UI 上都标记为 `deletable: true`，
/// 用户勾选后直接调用 `delete_filesystem_residual` → `remove_dir_all`。
pub fn scan_filesystem_residual(app_name: &str) -> Vec<FilesystemResidual> {
    let home = home_dir();
    let mut residuals = Vec::new();

    let local_appdata = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local"));
    let appdata = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Roaming"));

    let name_lower = app_name.to_lowercase();
    if name_lower.is_empty() {
        return residuals;
    }

    for base in [&local_appdata, &appdata] {
        if !base.is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let dir_name = entry.file_name().to_string_lossy().to_string();
                // 注意这里是 `remove_dir_all` 的唯一入口（见 delete_filesystem_residual）：
                // 没有确认框、不进回收站。误判 = 直接丢数据，所以走严格匹配。
                if is_residual_dir(&dir_name, &name_lower) {
                    let path = entry.path();
                    let size = dir_size(&path);
                    if size > 0 {
                        residuals.push(FilesystemResidual {
                            path: path.to_string_lossy().to_string(),
                            size,
                            deletable: true,
                            reason: String::new(),
                        });
                    }
                }
            }
        }
    }

    residuals
}

/// 删除单个文件系统残留目录
///
/// 返回 (是否成功, 消息)
pub fn delete_filesystem_residual(path: &str) -> (bool, String) {
    let p = Path::new(path);
    if !p.exists() {
        return (true, format!("路径已不存在: {}", path));
    }

    // 删除出口自带闸门：扫描侧的判定不能直接代表删除时刻的状态。
    // 目录可能在扫描之后被替换成符号链接/重解析点，remove_dir_all 顺着
    // 链接就会删到目标之外。
    if !is_deletable_fs_path(path) {
        return (
            false,
            format!("拒绝删除（形状不合法或属于共享容器）: {}", path),
        );
    }
    if let Ok(meta) = std::fs::symlink_metadata(p) {
        if meta.file_type().is_symlink() {
            return (false, format!("拒绝删除符号链接: {}", path));
        }
    }

    match std::fs::remove_dir_all(p) {
        Ok(_) => (true, format!("已删除: {}", path)),
        Err(e) => (false, format!("删除失败 {}: {}", path, e)),
    }
}

/// 扫描注册表残留
///
/// 在以下位置搜索与应用名匹配的注册表键：
/// - HKCU\SOFTWARE\* — 用户级应用设置（无需管理员权限可删除）
/// - HKLM\SOFTWARE\* — 系统级应用设置（需要管理员权限）
/// - HKLM\SOFTWARE\WOW6432Node\* — 32 位应用设置
///
/// 匹配规则：键名包含应用名（大小写不敏感）
pub fn scan_registry_residual(app_name: &str) -> Vec<RegistryResidual> {
    let mut items = Vec::new();
    let name_lower = app_name.to_lowercase();

    let search_roots = [
        (r"HKCU\SOFTWARE", false),
        (r"HKLM\SOFTWARE", true),
        (r"HKLM\SOFTWARE\WOW6432Node", true),
    ];

    for (root, is_system) in &search_roots {
        let output = Command::new("reg").args(["query", root]).output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for line in stdout.lines() {
                let line = line.trim();
                if !line.starts_with("HKEY") {
                    continue;
                }
                let key_name = line.rsplit('\\').next().unwrap_or("");

                // 这里命中就直接 `reg delete <key> /f`，且 **/f 跳过确认并递归删除
                // 整个子树**，所以必须走严格匹配，绝不用双向子串 / 首词。
                if is_residual_reg_key(key_name, &name_lower) {
                    let (deletable, reason) = if *is_system {
                        (false, "系统级注册表，需要管理员权限删除".to_string())
                    } else {
                        (true, String::new())
                    };

                    items.push(RegistryResidual {
                        key_path: line.to_string(),
                        is_system: *is_system,
                        deletable,
                        reason,
                    });
                }
            }
        }
    }

    items
}

/// 扫描环境变量残留
///
/// 检查以下环境变量是否引用了应用的安装路径：
/// - 用户环境变量: HKCU\Environment
/// - 系统环境变量: HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment
///
/// 特别处理 Path 变量：分割为条目列表，找出引用应用路径的条目。
/// 其他变量（JAVA_HOME、PYTHON_HOME 等）：如果值引用应用路径，整个变量标记为可删除。
pub fn scan_env_var_residual(app_name: &str, install_path: Option<&str>) -> Vec<EnvVarResidual> {
    let mut items = Vec::new();

    let install_path = match install_path {
        Some(p) if !p.is_empty() => p.to_lowercase(),
        _ => return items, // 没有安装路径无法匹配环境变量
    };

    // 搜索名称用于匹配变量值中的路径关键词。
    // 不能用带首词提取的 generate_search_names：卸载 "Go" 时 "go" 会命中
    // `C:\Program Files\Google\Chrome\...`，把 Chrome 的 PATH 条目删掉。
    let search_names = display_name_variants(app_name);

    let env_roots = [
        (r"HKCU\Environment", false),
        (
            r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            true,
        ),
    ];

    for (reg_key, is_system) in &env_roots {
        let output = Command::new("reg").args(["query", reg_key]).output();
        if let Ok(out) = output {
            let stdout = String::from_utf8_lossy(&out.stdout);

            for line in stdout.lines() {
                let line = line.trim();
                if line.is_empty() || !line.contains("REG_") {
                    continue;
                }

                // 解析环境变量名和值
                if let Some((var_name, var_value)) = parse_env_var_line(line) {
                    let value_lower = var_value.to_lowercase();

                    // Path 变量特殊处理：分割为条目
                    if var_name.eq_ignore_ascii_case("Path") {
                        let entries: Vec<&str> = var_value.split(';').collect();
                        let to_remove: Vec<String> = entries
                            .iter()
                            .filter(|e| {
                                let e_lower = e.to_lowercase();
                                // 条目引用了应用安装路径
                                e_lower.contains(&install_path)
                                // 或条目路径包含应用名关键词
                                || search_names.iter().any(|sn| e_lower.contains(sn) && e_lower.contains("program"))
                            })
                            .map(|e| e.to_string())
                            .collect();

                        if !to_remove.is_empty() {
                            let (deletable, reason) = if *is_system {
                                (false, "系统级 PATH，需要管理员权限修改".to_string())
                            } else {
                                (true, String::new())
                            };
                            items.push(EnvVarResidual {
                                var_name: var_name.clone(),
                                current_value: var_value.clone(),
                                entries_to_remove: to_remove,
                                is_system: *is_system,
                                deletable,
                                reason,
                            });
                        }
                    } else {
                        // 非 Path 变量：检查值是否引用应用路径
                        let references_app = value_lower.contains(&install_path)
                            || search_names.iter().any(|sn| {
                                value_lower.contains(sn) && value_lower.contains("\\program files")
                            });

                        if references_app {
                            let (deletable, reason) = if *is_system {
                                (false, "系统级环境变量，需要管理员权限修改".to_string())
                            } else {
                                (true, String::new())
                            };
                            items.push(EnvVarResidual {
                                var_name: var_name.clone(),
                                current_value: var_value.clone(),
                                entries_to_remove: vec![var_value.clone()],
                                is_system: *is_system,
                                deletable,
                                reason,
                            });
                        }
                    }
                }
            }
        }
    }

    items
}

/// 解析环境变量注册表行
///
/// 格式: "    JAVA_HOME    REG_SZ    C:\Program Files\Java\jdk1.8.0_291"
/// 或:   "    Path    REG_EXPAND_SZ    %SystemRoot%\system32;..."
fn parse_env_var_line(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.splitn(3, "    ").collect();
    if parts.len() >= 3 {
        let name = parts[0].trim().to_string();
        let value = parts[2].trim().to_string();
        if !name.is_empty() && !value.is_empty() {
            return Some((name, value));
        }
    }
    // 回退到 split_whitespace
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 3 {
        let type_idx = parts.iter().position(|p| p.starts_with("REG_"))?;
        if type_idx == 0 || type_idx >= parts.len() - 1 {
            return None;
        }
        let name = parts[..type_idx].join(" ");
        let value = parts[type_idx + 1..].join(" ");
        if !name.is_empty() {
            return Some((name, value));
        }
    }
    None
}

/// 删除注册表残留键
///
/// 使用 `reg delete <key> /f` 静默删除。
/// HKLM 下的键需要管理员权限，会返回失败信息。
pub fn delete_registry_residual(key_path: &str) -> (bool, String) {
    // 删除出口自带闸门。原先这里裸跑 `reg delete /f`，全部保护都活在扫描
    // 函数里，等于默认"扫的时候对 ⇒ 删的时候也对"。
    //
    // 用的是**档位一**（`is_deletable_reg_key_basic`）而不是版本化下钻那条
    // 严格的 `is_deletable_reg_path`：后者要求 SOFTWARE 下深度 ≥3，而当前
    // 扫描产出的合法目标就是一级子键，照搬会把整个功能挡死。
    if !is_deletable_reg_key_basic(key_path) {
        return (
            false,
            format!("拒绝删除注册表键（形状不合法或属于共享容器）: {}", key_path),
        );
    }

    let output = Command::new("reg")
        .args(["delete", key_path, "/f"])
        .output();

    match output {
        Ok(out) if out.status.success() => (true, format!("已删除注册表键: {}", key_path)),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = if stderr.is_empty() {
                "权限不足或键不存在".to_string()
            } else {
                stderr.trim().to_string()
            };
            (false, format!("删除注册表键失败 {}: {}", key_path, detail))
        }
        Err(e) => (false, format!("执行 reg delete 失败: {}", e)),
    }
}

/// 清理环境变量残留
///
/// 对于 Path 变量：移除引用应用路径的条目，保留其他条目。
/// 对于独立变量（JAVA_HOME 等）：删除整个变量。
///
/// 修改后广播 WM_SETTINGCHANGE 通知其他应用更新环境变量。
pub fn clean_env_var_residual(residual: &EnvVarResidual) -> (bool, String) {
    let reg_key = if residual.is_system {
        r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment"
    } else {
        r"HKCU\Environment"
    };

    if !residual.deletable {
        return (false, residual.reason.clone());
    }

    // Path 变量：移除特定条目
    if residual.var_name.eq_ignore_ascii_case("Path") {
        let entries: Vec<&str> = residual.current_value.split(';').collect();
        let to_remove_lower: Vec<String> = residual
            .entries_to_remove
            .iter()
            .map(|e| e.to_lowercase())
            .collect();

        let cleaned: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|e| !to_remove_lower.contains(&e.to_lowercase()))
            .collect();

        let new_path = cleaned.join(";");

        let output = Command::new("reg")
            .args([
                "add",
                reg_key,
                "/v",
                "Path",
                "/t",
                "REG_EXPAND_SZ",
                "/d",
                &new_path,
                "/f",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                broadcast_env_change();
                let removed_count = residual.entries_to_remove.len();
                (true, format!("已从 Path 移除 {} 个条目", removed_count))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                (false, format!("修改 Path 失败: {}", stderr.trim()))
            }
            Err(e) => (false, format!("执行 reg add 失败: {}", e)),
        }
    } else {
        // 独立变量：删除整个变量
        let output = Command::new("reg")
            .args(["delete", reg_key, "/v", &residual.var_name, "/f"])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                broadcast_env_change();
                (true, format!("已删除环境变量: {}", residual.var_name))
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                (
                    false,
                    format!("删除环境变量 {} 失败: {}", residual.var_name, stderr.trim()),
                )
            }
            Err(e) => (false, format!("执行 reg delete 失败: {}", e)),
        }
    }
}

/// 广播环境变量变更通知
///
/// 通过 PowerShell 广播 WM_SETTINGCHANGE 消息，
/// 让其他运行中的应用感知到环境变量已更新。
fn broadcast_env_change() {
    let ps_cmd = r#"
        Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @"
        [System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
        public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, IntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out IntPtr lpdwResult);
"@
        $HWND_BROADCAST = [IntPtr]0xffff
        $WM_SETTINGCHANGE = 0x1A
        $result = [IntPtr]::Zero
        [Win32.NativeMethods]::SendMessageTimeout($HWND_BROADCAST, $WM_SETTINGCHANGE, [IntPtr]::Zero, "Environment", 2, 5000, [ref]$result) | Out-Null
"#;

    let _ = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_cmd])
        .output();
}

/// 执行干净卸载（只卸载 + 扫描残留，不自动清理）
///
/// 完整流程：
/// 1. 运行应用自带的卸载程序
/// 2. 扫描注册表残留（不删除）
/// 3. 扫描环境变量残留（不删除）
/// 4. 扫描文件系统残留（不删除）
///
/// **用户选择权**：本函数只扫描残留，不自动清理。
/// 残留信息返回给调用方，由 UI 呈现给用户，
/// 用户确认后调用 `clean_all_residuals` 执行清理。
///
/// 返回 (卸载是否成功, 详细信息, 残留扫描结果)
pub fn clean_uninstall(
    uninstall_path: &str,
    app_name: &str,
    install_path: Option<&str>,
) -> (bool, String, UninstallResidual) {
    let mut residual = UninstallResidual {
        registry: Vec::new(),
        env_vars: Vec::new(),
        filesystem: Vec::new(),
    };

    crate::logger::info(&format!(
        "开始干净卸载: {} (路径: {}, 安装位置: {})",
        app_name,
        uninstall_path,
        install_path.unwrap_or("未知")
    ));

    // 1. 运行卸载程序
    crate::logger::info("步骤 1/4: 运行卸载程序");
    let (uninstall_ok, uninstall_msg) = uninstall_app(uninstall_path);
    if uninstall_ok {
        crate::logger::info(&format!("卸载程序执行成功: {}", uninstall_msg));
    } else {
        crate::logger::warn(&format!("卸载程序执行失败: {}", uninstall_msg));
    }

    // 无论卸载程序是否成功，都扫描残留
    // （有些卸载程序会失败但仍然删除了大部分文件）

    // 2. 扫描注册表残留（只扫描，不删除）
    crate::logger::info("步骤 2/4: 扫描注册表残留");
    residual.registry = scan_registry_residual(app_name);
    let reg_count = residual.registry.len();
    if reg_count > 0 {
        crate::logger::info(&format!(
            "发现 {} 个注册表残留项 (可删除 {} 个, 需管理员 {} 个)",
            reg_count,
            residual.registry.iter().filter(|r| r.deletable).count(),
            residual.registry.iter().filter(|r| !r.deletable).count()
        ));
    }

    // 3. 扫描环境变量残留（只扫描，不删除）
    crate::logger::info("步骤 3/4: 扫描环境变量残留");
    residual.env_vars = scan_env_var_residual(app_name, install_path);
    let env_count = residual.env_vars.len();
    if env_count > 0 {
        crate::logger::info(&format!(
            "发现 {} 个环境变量残留项 (可删除 {} 个, 需管理员 {} 个)",
            env_count,
            residual.env_vars.iter().filter(|e| e.deletable).count(),
            residual.env_vars.iter().filter(|e| !e.deletable).count()
        ));
    }

    // 4. 扫描文件系统残留（只扫描，不删除）
    crate::logger::info("步骤 4/4: 扫描文件系统残留");
    residual.filesystem = scan_filesystem_residual(app_name);
    let fs_count = residual.filesystem.len();
    if fs_count > 0 {
        let fs_size: u64 = residual.filesystem.iter().map(|f| f.size).sum();
        crate::logger::info(&format!(
            "发现 {} 个文件系统残留项, 总计 {}",
            fs_count,
            super::format_size(fs_size)
        ));
    }

    // 不自动清理！返回残留信息让用户决定
    let summary = format!(
        "{} | 残留扫描: 注册表 {} 项, 环境变量 {} 项, 文件 {} 项 (等待用户确认清理)",
        uninstall_msg, reg_count, env_count, fs_count
    );

    crate::logger::info(&format!(
        "干净卸载完成: 卸载={}, 残留 {} 项 (可删除 {} 项)",
        if uninstall_ok { "成功" } else { "失败" },
        residual.total_count(),
        residual.deletable_count()
    ));

    (uninstall_ok, summary, residual)
}

/// 清理所有可删除的残留（用户确认后调用）
///
/// 遍历残留列表，删除所有标记为 deletable=true 的项目。
/// 需要管理员权限的项（deletable=false）会被跳过。
///
/// 返回 (注册表已清理数, 环境变量已清理数, 文件系统已清理数)
pub fn clean_all_residuals(residual: &UninstallResidual) -> (usize, usize, usize) {
    crate::logger::info(&format!(
        "开始清理残留: 注册表 {} 项, 环境变量 {} 项, 文件 {} 项",
        residual.registry.len(),
        residual.env_vars.len(),
        residual.filesystem.len()
    ));

    // 1. 清理注册表残留
    let mut reg_cleaned = 0;
    for reg in &residual.registry {
        if reg.deletable {
            crate::logger::info(&format!("删除注册表键: {}", reg.key_path));
            let (ok, msg) = delete_registry_residual(&reg.key_path);
            if ok {
                reg_cleaned += 1;
                crate::logger::info(&format!("注册表键已删除: {}", reg.key_path));
            } else {
                crate::logger::warn(&format!("注册表键删除失败: {}", msg));
            }
        } else {
            crate::logger::warn(&format!("跳过注册表键（需管理员权限）: {}", reg.key_path));
        }
    }

    // 2. 清理环境变量残留
    let mut env_cleaned = 0;
    for env in &residual.env_vars {
        if env.deletable {
            crate::logger::info(&format!("清理环境变量: {}", env.var_name));
            let (ok, msg) = clean_env_var_residual(env);
            if ok {
                env_cleaned += 1;
                crate::logger::info(&format!("环境变量已清理: {} ({})", env.var_name, msg));
            } else {
                crate::logger::warn(&format!("环境变量清理失败: {} ({})", env.var_name, msg));
            }
        } else {
            crate::logger::warn(&format!("跳过环境变量（需管理员权限）: {}", env.var_name));
        }
    }

    // 3. 清理文件系统残留
    let mut fs_cleaned = 0;
    for fs in &residual.filesystem {
        if fs.deletable {
            crate::logger::info(&format!(
                "删除文件系统残留: {} ({})",
                fs.path,
                super::format_size(fs.size)
            ));
            let (ok, msg) = delete_filesystem_residual(&fs.path);
            if ok {
                fs_cleaned += 1;
                crate::logger::info(&format!("文件系统残留已删除: {}", msg));
            } else {
                crate::logger::warn(&format!("文件系统残留删除失败: {}", msg));
            }
        } else {
            crate::logger::warn(&format!("跳过文件系统残留（{}）: {}", fs.reason, fs.path));
        }
    }

    crate::logger::info(&format!(
        "残留清理完成: 注册表 {}/{} 项, 环境变量 {}/{} 项, 文件 {}/{} 项",
        reg_cleaned,
        residual.registry.len(),
        env_cleaned,
        residual.env_vars.len(),
        fs_cleaned,
        residual.filesystem.len()
    ));

    (reg_cleaned, env_cleaned, fs_cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_quoted() {
        let (exe, args) = parse_command(r#""C:\Program Files\App\uninstall.exe" /S"#);
        assert_eq!(exe, r"C:\Program Files\App\uninstall.exe");
        assert_eq!(args, vec!["/S"]);
    }

    #[test]
    fn test_parse_command_unquoted() {
        let (exe, args) = parse_command("uninstall.exe /quiet");
        assert_eq!(exe, "uninstall.exe");
        assert_eq!(args, vec!["/quiet"]);
    }

    /// 构建测试用 WindowsAppInfo
    fn make_app(
        name: &str,
        publisher: Option<&str>,
        install_loc: Option<&str>,
        system_component: bool,
    ) -> WindowsAppInfo {
        WindowsAppInfo {
            name: name.to_string(),
            install_location: install_loc.map(|s| s.to_string()),
            uninstall_string: None,
            quiet_uninstall_string: None,
            estimated_size: None,
            publisher: publisher.map(|s| s.to_string()),
            key_name: "test".to_string(),
            is_uwp: false,
            system_component,
        }
    }

    #[test]
    fn test_protection_critical() {
        // 规则 1: SystemComponent 标志
        assert_eq!(
            check_protection(&make_app(
                "Hidden System Update",
                Some("Microsoft"),
                None,
                true
            )),
            WinProtectionLevel::Critical
        );
        // 规则 2: 系统安装路径
        assert_eq!(
            check_protection(&make_app(
                "Some App",
                None,
                Some(r"C:\Windows\System32\app"),
                false
            )),
            WinProtectionLevel::Critical
        );
        assert_eq!(
            check_protection(&make_app(
                "Some App",
                None,
                Some(r"C:\Program Files\WindowsApps\test"),
                false
            )),
            WinProtectionLevel::Critical
        );
        // 规则 3: Microsoft 发布 + 系统关键词
        assert_eq!(
            check_protection(&make_app(
                "Windows Defender",
                Some("Microsoft Corporation"),
                None,
                false
            )),
            WinProtectionLevel::Critical
        );
        assert_eq!(
            check_protection(&make_app(
                "Microsoft Edge",
                Some("Microsoft Corporation"),
                None,
                false
            )),
            WinProtectionLevel::Critical
        );
        assert_eq!(
            check_protection(&make_app(
                "Microsoft .NET Framework 4.8",
                Some("Microsoft Corporation"),
                None,
                false
            )),
            WinProtectionLevel::Critical
        );
    }

    #[test]
    fn test_protection_official_uninstaller() {
        // 厂商关键词匹配应用名
        assert_eq!(
            check_protection(&make_app(
                "CrowdStrike Falcon",
                Some("CrowdStrike Inc."),
                None,
                false
            )),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
        // 厂商关键词匹配发布者
        assert_eq!(
            check_protection(&make_app(
                "Falcon Sensor",
                Some("CrowdStrike Inc."),
                None,
                false
            )),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
        // 安全类别关键词
        assert_eq!(
            check_protection(&make_app(
                "Acme Antivirus Pro",
                Some("Acme Corp"),
                None,
                false
            )),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
        assert_eq!(
            check_protection(&make_app(
                "Endpoint Protection Agent",
                Some("Unknown"),
                None,
                false
            )),
            WinProtectionLevel::RequiresOfficialUninstaller
        );
    }

    #[test]
    fn test_protection_data_protected() {
        // 密码管理器（类别关键词）
        assert_eq!(
            check_protection(&make_app("1Password", Some("AgileBits"), None, false)),
            WinProtectionLevel::DataProtected
        );
        assert_eq!(
            check_protection(&make_app(
                "KeePass Password Safe",
                Some("Dominik Reichl"),
                None,
                false
            )),
            WinProtectionLevel::DataProtected
        );
        // 输入法（类别关键词）
        assert_eq!(
            check_protection(&make_app("搜狗输入法", Some("Sogou"), None, false)),
            WinProtectionLevel::DataProtected
        );
        assert_eq!(
            check_protection(&make_app("小狼毫输入法", Some("RIME"), None, false)),
            WinProtectionLevel::DataProtected
        );
    }

    #[test]
    fn test_protection_none() {
        assert_eq!(
            check_protection(&make_app(
                "Visual Studio Code",
                Some("Microsoft Corporation"),
                None,
                false
            )),
            WinProtectionLevel::None
        );
        // 注意：VS Code 虽然是 Microsoft 发布，但名称不含系统关键词，所以不是 Critical
        assert_eq!(
            check_protection(&make_app(
                "Spotify",
                Some("Spotify AB"),
                Some(r"C:\Users\test\AppData\Local\Spotify"),
                false
            )),
            WinProtectionLevel::None
        );
    }

    #[test]
    fn test_is_security_app_vendor_in_publisher() {
        // 厂商名在 publisher 中但不在应用名中
        let app = make_app(
            "Falcon Platform",
            Some("CrowdStrike Holdings, Inc."),
            None,
            false,
        );
        assert!(is_security_app(&app));
    }

    #[test]
    fn test_is_critical_non_microsoft_app() {
        // 非 Microsoft 发布的应用即使名称含 "windows" 也不是 Critical
        let app = make_app(
            "Windows Media Player Classic",
            Some("Some Random Corp"),
            None,
            false,
        );
        assert!(!is_critical_system_app(&app));
    }

    #[test]
    fn test_critical_windows_subdir_in_program_files() {
        // C:\Program Files\Windows Defender\ 应被识别为系统路径
        assert_eq!(
            check_protection(&make_app(
                "Windows Defender",
                None,
                Some(r"C:\Program Files\Windows Defender"),
                false
            )),
            WinProtectionLevel::Critical
        );
        // C:\Program Files\Windows NT\Accessories\ 应被识别为系统路径
        assert_eq!(
            check_protection(&make_app(
                "WordPad",
                None,
                Some(r"C:\Program Files\Windows NT\Accessories"),
                false
            )),
            WinProtectionLevel::Critical
        );
        // 32 位路径
        assert_eq!(
            check_protection(&make_app(
                "Some Tool",
                None,
                Some(r"C:\Program Files (x86)\Windows Kits\10"),
                false
            )),
            WinProtectionLevel::Critical
        );
    }

    #[test]
    fn test_d_drive_app_not_critical() {
        // D 盘安装的应用不应被识别为系统关键
        let app = make_app("My App", Some("Some Corp"), Some(r"D:\Apps\MyApp"), false);
        assert!(!is_critical_system_app(&app));
        assert_eq!(check_protection(&app), WinProtectionLevel::None);

        // E 盘同理
        let app = make_app("Game", Some("Game Studio"), Some(r"E:\Games\Game"), false);
        assert!(!is_critical_system_app(&app));

        // 自定义路径
        let app = make_app("Tool", Some("Tool Inc"), Some(r"C:\MyTools\Tool"), false);
        assert!(!is_critical_system_app(&app));
    }

    #[test]
    fn test_program_files_not_all_critical() {
        // C:\Program Files\ 下的普通第三方应用不应是 Critical
        let app = make_app(
            "Spotify",
            Some("Spotify AB"),
            Some(r"C:\Program Files\Spotify"),
            false,
        );
        assert!(!is_critical_system_app(&app));
        assert_eq!(check_protection(&app), WinProtectionLevel::None);
    }

    #[test]
    fn test_detect_silent_flags_inno_setup() {
        // Inno Setup 卸载程序通常名为 unins000.exe
        let flags = detect_silent_flags(r"C:\Program Files\App\unins000.exe", &[]);
        assert!(flags.contains(&"/VERYSILENT"));
        assert!(flags.contains(&"/SUPPRESSMSGBOXES"));
    }

    #[test]
    fn test_detect_silent_flags_nsis_default() {
        // 默认使用 NSIS 的 /S
        let flags = detect_silent_flags(r"C:\Program Files\App\uninstall.exe", &[]);
        assert_eq!(flags, vec!["/S"]);
    }

    #[test]
    fn test_detect_silent_flags_d_drive() {
        // D 盘的 Inno Setup 卸载程序也能正确检测
        let flags = detect_silent_flags(r"D:\Apps\MyApp\unins000.exe", &[]);
        assert!(flags.contains(&"/VERYSILENT"));
    }

    #[test]
    fn test_parse_command_d_drive() {
        // D 盘带空格路径
        let (exe, args) = parse_command(r#""D:\My Apps\Test\uninstall.exe" /S"#);
        assert_eq!(exe, r"D:\My Apps\Test\uninstall.exe");
        assert_eq!(args, vec!["/S"]);
    }


    #[test]
    fn test_parse_env_var_line() {
        // 标准格式
        let (name, value) =
            parse_env_var_line("    JAVA_HOME    REG_SZ    C:\\Program Files\\Java\\jdk1.8.0_291")
                .unwrap();
        assert_eq!(name, "JAVA_HOME");
        assert_eq!(value, r"C:\Program Files\Java\jdk1.8.0_291");

        // REG_EXPAND_SZ 格式
        let (name, value) =
            parse_env_var_line("    Path    REG_EXPAND_SZ    %SystemRoot%\\system32;C:\\Python39")
                .unwrap();
        assert_eq!(name, "Path");
        assert_eq!(value, r"%SystemRoot%\system32;C:\Python39");
    }

    #[test]
    fn test_parse_env_var_line_invalid() {
        // 空行
        assert!(parse_env_var_line("").is_none());
        // 无 REG_ 类型
        assert!(parse_env_var_line("JAVA_HOME    value").is_none());
    }

    #[test]
    fn test_uninstall_residual_empty() {
        let residual = UninstallResidual {
            registry: Vec::new(),
            env_vars: Vec::new(),
            filesystem: Vec::new(),
        };
        assert!(residual.is_empty());
        assert_eq!(residual.total_count(), 0);
    }

    #[test]
    fn test_uninstall_residual_non_empty() {
        let residual = UninstallResidual {
            registry: vec![RegistryResidual {
                key_path: "HKCU\\SOFTWARE\\TestApp".to_string(),
                is_system: false,
                deletable: true,
                reason: String::new(),
            }],
            env_vars: vec![EnvVarResidual {
                var_name: "TEST_HOME".to_string(),
                current_value: "C:\\Test".to_string(),
                entries_to_remove: vec!["C:\\Test".to_string()],
                is_system: false,
                deletable: true,
                reason: String::new(),
            }],
            filesystem: vec![FilesystemResidual {
                path: "C:\\Test".to_string(),
                size: 1024,
                deletable: true,
                reason: String::new(),
            }],
        };
        assert!(!residual.is_empty());
        assert_eq!(residual.total_count(), 3);
        assert_eq!(residual.deletable_count(), 3);
    }

    #[test]
    fn test_registry_residual_system_not_deletable() {
        // HKLM 下的注册表键标记为不可删除（需要管理员权限）
        // 这里只测试数据结构，不实际调用 reg query
        let residual = RegistryResidual {
            key_path: r"HKLM\SOFTWARE\TestApp".to_string(),
            is_system: true,
            deletable: false,
            reason: "系统级注册表，需要管理员权限删除".to_string(),
        };
        assert!(!residual.deletable);
        assert!(residual.is_system);
    }

    #[test]
    fn test_env_var_residual_path_type() {
        // Path 类型的环境变量残留：只移除特定条目
        let residual = EnvVarResidual {
            var_name: "Path".to_string(),
            current_value: r"C:\Windows\system32;C:\Python39;C:\Other".to_string(),
            entries_to_remove: vec![r"C:\Python39".to_string()],
            is_system: false,
            deletable: true,
            reason: String::new(),
        };
        assert_eq!(residual.entries_to_remove.len(), 1);
        // 验证清理后保留的条目
        let entries: Vec<&str> = residual.current_value.split(';').collect();
        let to_remove_lower: Vec<String> = residual
            .entries_to_remove
            .iter()
            .map(|e| e.to_lowercase())
            .collect();
        let cleaned: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|e| !to_remove_lower.contains(&e.to_lowercase()))
            .collect();
        assert_eq!(cleaned.join(";"), r"C:\Windows\system32;C:\Other");
    }

    #[test]
    fn test_is_windows_system_name() {
        assert!(is_windows_system_name("Microsoft"));
        assert!(is_windows_system_name("microsoft"));
        assert!(is_windows_system_name("Windows"));
        assert!(is_windows_system_name("Packages"));
        assert!(is_windows_system_name("CrashDumps"));
        assert!(is_windows_system_name("Adobe"));
        assert!(is_windows_system_name("Macromedia"));
        assert!(!is_windows_system_name("MyUninstalledApp"));
        assert!(!is_windows_system_name("Spotify"));
    }

    #[test]
    fn test_is_windows_app_installed() {
        let mut installed = HashSet::new();
        installed.insert("spotify".to_string());
        installed.insert("mozilla firefox".to_string());
        installed.insert("visual studio code".to_string());
        installed.insert("epic games launcher".to_string());

        // 精确匹配
        assert!(is_windows_app_installed("Spotify", &installed));
        // 目录名是已安装应用名的子串
        assert!(is_windows_app_installed("Mozilla", &installed));
        assert!(is_windows_app_installed("Firefox", &installed));
        // 紧凑名匹配
        assert!(is_windows_app_installed("VisualStudioCode", &installed));
        assert!(is_windows_app_installed("EpicGamesLauncher", &installed));
        // 未安装应用不匹配
        assert!(!is_windows_app_installed("UnknownApp", &installed));
        assert!(!is_windows_app_installed("FooBar", &installed));
    }
}
