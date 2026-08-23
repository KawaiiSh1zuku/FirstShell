import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

function App() {
  return (
    <main className="workspace">
      <header className="titlebar">
        <strong>FirstShell</strong>
        <span>本地工作区</span>
      </header>
      <aside className="connections" aria-label="连接">
        <div className="panel-title">连接</div>
      </aside>
      <section className="terminal" aria-label="终端工作区">
        <div className="tabbar"><span className="tab">欢迎</span></div>
        <div className="empty-terminal">FirstShell</div>
      </section>
      <aside className="inspector" aria-label="会话信息">
        <div className="panel-title">会话</div>
      </aside>
      <footer className="statusbar">
        <span>未连接</span><span>M0</span>
      </footer>
    </main>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("missing root element");
createRoot(root).render(<StrictMode><App /></StrictMode>);
