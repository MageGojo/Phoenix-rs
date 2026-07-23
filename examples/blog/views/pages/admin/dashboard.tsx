import type { AdminDashboardProps } from "../../generated/contracts.js";

export default function AdminDashboard({
  users,
  auditEvents,
  activeSessions,
  pendingPasswordResets,
}: AdminDashboardProps) {
  const lockedUsers = users.filter((user) => user.locked).length;

  return (
    <main className="admin-shell">
      <header className="page-heading">
        <p className="kicker">Management Console</p>
        <h1>管理后台</h1>
        <p>集中查看用户、角色、会话和审计事件。</p>
      </header>

      <section className="metrics" aria-label="后台指标">
        <div><span>用户</span><strong>{users.length}</strong></div>
        <div><span>活跃会话</span><strong>{activeSessions}</strong></div>
        <div><span>待处理重置</span><strong>{pendingPasswordResets}</strong></div>
        <div><span>锁定账号</span><strong>{lockedUsers}</strong></div>
      </section>

      <section className="directory-panel" aria-labelledby="admin-users-title">
        <div className="result-bar">
          <h2 id="admin-users-title">用户与角色</h2>
          <span>RBAC 管理入口首版</span>
        </div>
        <div className="table-wrap">
          <table>
            <thead>
              <tr><th>用户</th><th>邮箱</th><th>角色</th><th>状态</th></tr>
            </thead>
            <tbody>
              {users.map((user) => (
                <tr key={user.id}>
                  <td>{user.name}</td>
                  <td>{user.email}</td>
                  <td>{user.role}</td>
                  <td>{user.locked ? "锁定" : "正常"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="directory-panel" aria-labelledby="audit-title">
        <div className="result-bar">
          <h2 id="audit-title">审计日志</h2>
          <span>最近 {auditEvents.length} 条事件</span>
        </div>
        <ul>
          {auditEvents.map((event) => (
            <li key={event.id}>
              <strong>{event.action}</strong> · {event.actor} · {event.subject} · {event.occurredAt}
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}
