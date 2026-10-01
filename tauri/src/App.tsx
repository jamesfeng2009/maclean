import { AppProvider, useApp } from "./lib/store";
import { Rail } from "./components/Rail";
import { TopBar } from "./components/TopBar";
import { ToastHost } from "./components/ToastHost";
import { ScanOverlay } from "./components/ScanOverlay";
import { ConfirmModal } from "./components/ConfirmModal";
import { Overview } from "./pages/Overview";
import { Analysis } from "./pages/Analysis";
import { Clean } from "./pages/Clean";
import { Dup } from "./pages/Dup";
import { Uninstall } from "./pages/Uninstall";
import { Startup } from "./pages/Startup";
import { Optimize } from "./pages/Optimize";
import { Settings } from "./pages/Settings";

function Pages() {
  const { page } = useApp();
  // key 让页面在切换时保留各自的扫描状态（不强制 remount）
  switch (page) {
    case "overview":
      return <Overview />;
    case "analysis":
      return <Analysis />;
    case "clean":
      return <Clean />;
    case "dup":
      return <Dup />;
    case "uninstall":
      return <Uninstall />;
    case "startup":
      return <Startup />;
    case "optimize":
      return <Optimize />;
    case "settings":
      return <Settings />;
  }
}

function Shell() {
  return (
    <div className="app">
      <Rail />
      <div className="main">
        <TopBar />
        <main className="content" key="content">
          <Pages />
        </main>
      </div>
      <ScanOverlay />
      <ConfirmModal />
      <ToastHost />
    </div>
  );
}

export default function App() {
  return (
    <AppProvider>
      <Shell />
    </AppProvider>
  );
}
