# maclean 遗留三问 · 修复建议

> 对应 `CODE_REVIEW.md` 末尾「未决项」。本文给结论 + 依据 + 可落地代码 + 代价，不替 Jeffery 做产品决策。
> 代码证据基于 2026-09-14 工作区状态：`edition = "2021"`、`eframe 0.29`、8 个 `static mut`。

---

## 0. 结论速览

| # | 问题 | 建议 | 是否需你拍板 | 工作量 |
|---|------|------|------------|-------|
| 1 | `.env` 后门 | Cargo feature `dev-bypass` + `build.rs` 熔断 + DEV 角标，**不用 `cfg(debug_assertions)`** | 是（接受何种代价） | 1h |
| 2 | 7 个 `static mut` | **不需要 `impl eframe::App` 大重构** —— 闭包是 `FnMut`，直接捕获 `GuiState` | 否，纯技术 | 2h |
| 3 | License 无 exp/吊销 | 分三层：**L1 纯离线过期**（现在就做）/ **L2 签名静态吊销表**（推荐，零后端）/ L3 接 Paddle | 是（是否上吊销） | L1 半天 / L2 一天 |

---

## 1. `.env` 后门 —— 不要用 `cfg(debug_assertions)`

### 1.1 先纠一个事实：风险面比"环境变量"更大

`is_dev_mode()` 只有 **4 个调用点**（`license.rs:260`、`license.rs:361`、`app.rs:983`、`main.rs:8078`），
但真正的泄漏面在 `main.rs:104` 的 `load_dotenv()`：

```rust
let cwd = std::env::current_dir()?;   // ← 读的是「当前工作目录」
let path = cwd.join(".env");
```

也就是说**任意目录下放一个 `.env` 就能解锁**。发布成 `.app` 后 CWD 通常是 `/`，风险不算高；
但配合下面这条就完全不一样了：

```
$ strings target/release/maclean | grep -i maclean_dev
MACLEAN_DEV
```

**变量名是明文躺在二进制里的**，这是收入泄漏，不是"开发者才知道的开关"。
只要有一个人把 `MACLEAN_DEV=1 ./maclean` 发到小红书，你的付费墙就没了。

### 1.2 为什么不建议 `cfg(debug_assertions)`

三条理由：

1. **语义错**。它表达的是"这是未优化的构建"，而你要表达的是"我授权这台机器免检"。
   拿构建类型冒充授权豁免，第一次为了让 UI 跑得动而用 `--release` 调测时就会自打脸。
2. **代价不对称**。绑 debug 会失去 `make dev-release`；绑 feature 只需把这一行改掉：
   ```make
   dev-release:
   	MACLEAN_ALLOW_DEV_RELEASE=1 cargo build --release --features dev-bypass
   	./target/release/maclean
   ```
   能力不丢，只是多敲一次参数。
3. **2024 edition 会连带炸**。`std::env::set_var`（`main.rs:120`）在 edition 2024 变成 `unsafe`，
   和 `static mut` 是同一批硬错误。一起处理比分两次划算。

### 1.3 建议方案：feature + 编译期熔断 + 视觉角标

**Cargo.toml**

```toml
[features]
# 开发者免授权构建。**任何对外发布的产物都不得开启此 feature。**
dev-bypass = []
```

**src/license.rs** —— 用 `#[cfg]`（不是 `cfg!()`），让代码在正式包里**根本不存在**：

```rust
/// 是否为开发者免授权构建。
///
/// 只有显式 `--features dev-bypass` 时才可能为 true。
/// 正式构建下此函数被完全编译掉，`strings` 里不应出现任何 dev 相关字符串。
#[cfg(feature = "dev-bypass")]
pub fn is_dev_mode() -> bool {
    // feature 已开，再用 env 做二级开关，方便同一份二进制切换行为
    !std::env::var("MACLEAN_DEV_DISABLE").is_ok()
}

#[cfg(not(feature = "dev-bypass"))]
pub fn is_dev_mode() -> bool {
    false
}
```

`#[cfg]` 而非 `cfg!()` 是关键：`cfg!(feature)` 会留下 `if false { ... "dev@maclean.app" ... }`，
字符串表在 LTO 下**不保证**被清除；`#[cfg]` 是真的不编译。

**build.rs** —— 防止手滑把后门编进 release，留一个显式逃生阀：

```rust
fn main() {
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let dev_bypass = std::env::var("CARGO_FEATURE_DEV_BYPASS").is_ok();
    let allow = std::env::var("MACLEAN_ALLOW_DEV_RELEASE").as_deref() == Ok("1");

    if dev_bypass && profile == "release" && !allow {
        panic!(
            "\n\n  拒绝构建：release + dev-bypass 同时开启。\n  \
             若确为本地调试，请显式声明：\n    \
             MACLEAN_ALLOW_DEV_RELEASE=1 cargo build --release --features dev-bypass\n"
        );
    }
    println!("cargo:rerun-if-env-changed=MACLEAN_ALLOW_DEV_RELEASE");
}
```

**main.rs:104 `load_dotenv`** —— 收两个口子：只在 dev 构建里加载，且只认项目目录。

```rust
#[cfg(feature = "dev-bypass")]
fn load_dotenv() {
    // 只从 CARGO_MANIFEST_DIR 读，绝不读 CWD
    let Ok(path) = std::env::var("CARGO_MANIFEST_DIR") else { return };
    let Ok(content) = std::fs::read_to_string(
        std::path::Path::new(&path).join(".env")) else { return };
    for line in content.lines() { /* 解析逻辑不变 */ }
}

#[cfg(not(feature = "dev-bypass"))]
fn load_dotenv() {}
```

**UI DEV 角标**（`app.rs:983` 附近已有 dev 分支）—— dev 构建在标题栏挂一个红底 `DEV BUILD`，
避免把 dev 包当正式包测、更避免误发。

**CI 兜底**（加到 `ci.yml` 的 `macos-build` job）：

```yaml
      - name: 断言发布构建无 dev 后门
        run: |
          cargo build --release
          if strings target/release/maclean | grep -q "MACLEAN_DEV"; then
            echo "错误：release 产物含 dev 后门字符串" >&2; exit 1
          fi
```

### 1.4 代价

- `make dev-release` 多一个环境变量 + feature 参数（写进 Makefile 后对用户无感）。
- dev 与正式二进制**不再是同一份**，测试结论不能跨构建迁移 —— 这本来就是应该的。

---

## 2. 7 个 `static mut` —— 推翻"要迁 `impl eframe::App`"的结论

### 2.1 证据：48 处引用全部落在一段 414 行的闭包体内

```
main.rs:154   eframe::run_simple_native("Maclean", options, move |ctx, _frame| {
main.rs:154-161   7 个 static mut 定义
main.rs:265-678   48 处引用（APP 20 / SCAN_RX 7 / DELETE_RX 7 / MENUBAR 4 /
                  AUTO_CLEAN_AFTER_SCAN 6 / NEEDS_INIT 2 / LAST_DISK_UPDATE 2）
main.rs:678+      其余 79 个 fn，零引用
```

**没有任何一个嵌套 `fn` 或外部函数引用这些全局**。所以这不是"结构性重构"，是**一段局部代码的机械替换**。
我上一轮说"要把状态迁进 `Mutex` / `impl eframe::App`" —— 过头了。

### 2.2 关键事实：`run_simple_native` 的闭包本来就是 `FnMut`

```rust
pub fn run_simple_native(
    app_name: &str,
    native_options: NativeOptions,
    update_fun: impl FnMut(&Context, &mut Frame) + 'static,   // ← FnMut
)
```

`FnMut` 意味着**闭包可以捕获并修改外部状态**。原来的 `static mut` 从一开始就完全没必要 ——
作者大概是照抄了「egui immediate mode 里状态放哪」的旧写法。直接捕获即可，不需要 `impl eframe::App`。

### 2.3 落地代码

```rust
// main.rs，GUI 启动处

struct GuiState {
    app: App,
    scan_rx: Option<mpsc::Receiver<ScanMessage>>,
    delete_rx: Option<mpsc::Receiver<DeleteMessage>>,
    menubar: Option<menubar::MenuBarHud>,
    needs_init: bool,
    last_disk_update: f64,
    auto_clean_after_scan: bool,
}

let mut state = GuiState {
    app: App::new(),
    scan_rx: None,
    delete_rx: None,
    menubar: None,
    needs_init: true,
    last_disk_update: 0.0,
    auto_clean_after_scan: false,
};

eframe::run_simple_native("Maclean", options, move |ctx, _frame| {
    // 原 265–678 行闭包体，机械替换见下表
});
```

替换映射（48 处，全部可脚本化）：

| 原写法 | 新写法 | 出现次数 |
|--------|--------|---------|
| `static mut X: Option<T> = None;` | 删除，进 `GuiState` | 4 |
| `unsafe { AUTO_CLEAN_AFTER_SCAN = false; ... }` | `{ state.auto_clean_after_scan = false; ... }` | 6 |
| `unsafe { if NEEDS_INIT { APP = Some(App::new()); ... } }` | `if state.needs_init { state.app = App::new(); ... }` | 2 |
| `start_scan(app, &mut SCAN_RX)` | `start_scan(&mut state.app, &mut state.scan_rx)` | 7 |
| `if let Some(app) = &mut APP` | `let app = &mut state.app;` | 20 |
| `if let Some(ref mut mb) = MENUBAR` | `if let Some(mb) = state.menubar.as_mut()` | 4 |
| `DELETE_RX` / `LAST_DISK_UPDATE` | `state.delete_rx` / `state.last_disk_update` | 9 |

**辅助函数签名一个都不用改**（`start_scan(&mut App, &mut Option<Receiver<..>>)` 原样保留）。

注意一个坑：`render_hud_window(ctx, &mut state.app)` 这类嵌套 `fn` 若同时需要 `state.scan_rx`，
会产生 `&mut state.app` + `&mut state.scan_rx` 的**双字段借用** —— Rust 允许不相交字段的同时借用，
写成 `render_hud_window(ctx, &mut state.app)` 与 `start_scan(&mut state.app, &mut state.scan_rx)`
在同一表达式里会冲突，需拆成两条语句。遇到时按编译器报错拆即可，不是设计问题。

### 2.4 顺带处理另外两处

**`sudo_keepalive.rs:17`**（独立于 GUI，2 分钟）：

```rust
static KEEPALIVE_STATE: Mutex<Option<KeepaliveState>> = Mutex::new(None);
```

`Mutex::new` 是 const fn，Rust 1.63+ 直接可用，不需要 `OnceLock`。
调用点已有 `KEEPALIVE_INIT.lock().unwrap()`，把访问包进同一个临界区即可。

**`aewp.rs` 的 15 处 `unsafe`** —— **不要动**。那是 `AuthorizationExecuteWithPrivileges` /
`AuthorizationCreate` 的 FFI 调用，属于合法 unsafe。只需给每个块补一行 `// SAFETY:` 注释说明
不变量（句柄非空、字符串以 NUL 结尾、成对 Free）。clippy 不会报这些。

### 2.5 收尾

改完后把 `ci.yml` 里这段删掉：

```yaml
      - name: Clippy 静态检查
        continue-on-error: true          # ← 删掉这一行
        run: cargo clippy --all-targets -- -D warnings
```

### 2.6 代价

- ~2 小时机械改动，风险集中在「漏改一处导致状态错乱」，靠 `cargo check` + 手动点一遍 UI 覆盖。
- **不做** `impl eframe::App` 迁移：`main.rs` 8508 行拆模块是另一个独立议题，不该和这件事绑一起。

---

## 3. License 无 exp / 无吊销 —— 不需要后端，只需要一个签名的静态文件

### 3.1 现状盘点

`LicensePayload` 只有 4 个字段（`license.rs:40-51`）：

```rust
pub struct LicensePayload {
    pub email: String,
    pub plan: String,     // "lifetime" / "yearly" —— 但没有任何代码读它
    pub iat: u64,         // 签发时间，只写不校验
    pub mid: String,      // 机器绑定
}
```

三个后果：
- `plan` 是装饰品，`yearly` 永远不会过期；
- 没有 key id，无法吊销（想拉黑一个退款用户，只能换公钥 → 所有老用户一起失效）；
- `quota.json` / `activation.json` 是明文 JSON，改一个数字就无限白嫖。

### 3.2 L1：纯离线过期（半天，立刻能做）

payload 升到 v2，**新字段全部 `#[serde(default)]`，老 key 不受影响**：

```rust
pub struct LicensePayload {
    pub email: String,
    pub plan: String,
    pub iat: u64,
    #[serde(default)] pub mid: String,
    #[serde(default)] pub v: u8,        // schema 版本，缺省 1
    #[serde(default)] pub exp: u64,     // 过期时间（Unix 秒），0 = 永不过期
    #[serde(default)] pub jti: String,  // key id，吊销用
}
```

校验（追加到 `verify_license` 末尾）：

```rust
if payload.exp != 0 {
    let now = monotone_now();           // 见下
    if now > payload.exp {
        return Err("License 已过期，请续期".to_string());
    }
}
```

**时钟回滚对抗**（不改系统时间就能绕是最容易踩的坑）：

```rust
/// 单调时钟：取「系统时间」与「上次观测时间」的较大值。
/// 用户把系统时间调回过去不会让 exp 变远，只会让 last_seen 停住。
fn monotone_now() -> u64 {
    let sys = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let last = ACTIVATION_CACHE.lock().unwrap().last_seen;   // 每次启动回写
    let now = sys.max(last);
    ACTIVATION_CACHE.lock().unwrap().last_seen = now;
    now
}
```

`keygen.rs` 同步加 `--exp`（`yearly` 默认 `iat + 365d`，`lifetime` 不写），并把 `jti` 用 UUID 生成、
在终端打印出来 —— **从现在开始，每一把签出的 key 都要把 jti 记进销售台账**，否则 L2 无从吊销。

> L1 无法防「重装系统 + 全新激活」，但能防住 99% 的"买一年用一辈子"。

### 3.3 L2：签名静态吊销表（推荐，一天，零后端）

核心思路：**吊销表不需要服务器，只需要一个用同一把 ed25519 私钥签名的静态 JSON 文件**。
你已经有 `sunge.men` + Cloudflare，而且 `updater.rs:33` 已经在用 `curl` 抓 GitHub API —— 通路是现成的。

**服务端**（就是一次静态文件发布）：

```
https://sunge.men/maclean/revoked.json
```

内容与 License 同构，直接复用现有验证代码：

```json
{
  "payload": "MACL-<base64url(payload)>-<base64url(sig)>"
}
```

payload：

```json
{"v":1,"iat":1784300000,"revoked":["jti-xxx","jti-yyy"],"min_version":"0.2.0"}
```

改一次就重新签一次、推一次 CF 缓存。没有数据库，没有 API，没有运维。

**客户端**（`license.rs` 新增，全部在后台线程）：

1. 启动 → `curl -sS --max-time 5 https://sunge.men/maclean/revoked.json`
2. `verify_license(&raw)` 验签（**必须验**，否则任何人做 MITM 就能吊销所有人的 key）
3. 成功 → 写入 `~/Library/Application Support/maclean/revoked.json`，记 `last_check = now`
4. 失败 → **完全静默**，用本地缓存继续跑；不弹窗、不阻断
5. 校验：本地 `jti ∈ revoked` → 降级 Free，提示"此 License 已被撤销"

**离线宽限**（关键设计，别锁死出差用户）：

```
days = now - last_check
days <= 30            → 正常（宽容期，完全信任缓存）
30 < days <= 90       → 正常，但在设置页显示"建议联网验证一次"
days >  90            → 降级 Free，弹一次说明："需要联网验证授权"
```

30/90 两个数字你定，我建议这个量级 —— 太短会误伤，太长等于没有吊销。

### 3.4 L3：什么时候才需要真后端

只有当你做**订阅制**（月付/自动续期）时才需要接 Lemon Squeezy / Paddle ——
它们自带 license key API、吊销、续期、发票、退税处理。自己写这套是重复造轮子且容易在税务上翻车。
**买断制走到 L2 就够了。**

### 3.5 顺带修两个小坑

**机器绑定硬失败**（`license.rs:276`）：

```rust
if record.machine_hash != machine_hash() { return LicenseStatus::Free; }
```

换机 / 重装系统的用户会直接变成免费版，这是差评来源。建议 `activation.json` 里加 `last_rebind`，
允许**每 90 天自动重绑 1 次**，超频则要求走 L2 的吊销表放行或人工支持。

**quota 明文篡改**：`quota.json` 改个数字就满血。彻底防不住（用户拥有本机），
可以做个便宜加固：把 `used_bytes` 用 `HMAC(machine_hash + 内置常量)` 签名，
改数字得同时改机器指纹 —— 而改机器指纹会让 License 失效。把成本从"记事本改一行"抬到"要逆向"。
**但别在这上面投太多**：额度只拦轻度白嫖，真正的闸门是"超过 500MB 必须激活"。

---

## 4. 建议执行顺序

| 序 | 事项 | 依赖 | 阻塞谁 |
|----|------|------|--------|
| 1 | #2 `static mut` → `GuiState` | 无 | CI 摘掉 `continue-on-error` |
| 2 | #1 dev-bypass feature 化 | 无 | 公开发布 |
| 3 | #3-L1 payload v2 + exp + jti | 无 | 3 依赖它 |
| 4 | #3-L2 吊销表 | 待 #3-L1 出 jti；需你提供 `sunge.men` 路径与签发流程 | 退款 / 漏发管理 |
| 5 | #3.5 机器重绑 + quota HMAC | 无 | 换机用户体验 |

前三项不依赖你的任何决策，我可以直接动手。第 4 项需要你确认两件事：
**吊销表放哪个域名路径**、**keygen 签出的 jti 你打算怎么记账**（哪怕只是个 CSV 也行）。
