import { ListTodo } from "lucide-react";
import appStyles from "../App.module.css";
import { EmptyState } from "../views/EmptyState";

export function CloudApp() {
  return (
    <div className={appStyles.app}>
      <header className={appStyles.header}>
        <div className={appStyles.title}>Fleet Cloud</div>
        <span className={appStyles.connDot} data-state="offline" aria-hidden="true" />
        <span className={appStyles.connLabel}>等待 Cloud API</span>
      </header>
      <main className={appStyles.main}>
        <EmptyState
          icon={ListTodo}
          title="还没有云任务"
          description="CloudFleetClient 接入后，Task 与每次 Attempt 会在这里形成连续记录。"
        />
      </main>
    </div>
  );
}
