import { useApp, PAGE_LABEL } from "../lib/store";
import { Icon } from "./Icon";

export function TopBar() {
  const { page, theme, toggleTheme, langEn, setLangEn, toast } = useApp();
  return (
    <header className="topbar">
      <div className="crumb">
        maclean <span style={{ margin: "0 6px", color: "var(--text-3)" }}>/</span>{" "}
        <b>{PAGE_LABEL[page]}</b>
      </div>
      <div className="grow" />
      <button className="btn" onClick={() => toast("info", "搜索：骨架阶段暂未开放")}>
        <Icon name="search" size={15} /> 搜索
      </button>
      <button className="btn" onClick={toggleTheme}>
        <Icon name={theme === "dark" ? "sun" : "moon"} size={15} />
        {theme === "dark" ? "浅色" : "深色"}
      </button>
      <button
        className="btn"
        onClick={() => setLangEn(!langEn)}
        title="切换中文 / English"
      >
        {langEn ? "EN" : "中"} / {langEn ? "中" : "EN"}
      </button>
    </header>
  );
}
