import MemberCreator from "../../islands/member-creator.js";
import type { Member } from "../../types/member.js";

export interface MembersIndexProps {
  members: Member[];
  generatedBy: string;
  total: number;
}

export default function MembersIndex({ members, generatedBy, total }: MembersIndexProps) {
  const activeCount = members.filter((member) => member.status === "active").length;
  const projectCount = members.reduce((sum, member) => sum + member.projects, 0);
  const visibleMembers = members.slice(0, 10);

  return (
    <div className="directory-shell">
      <a className="skip-link" href="#member-table">跳到成员列表</a>
      <header className="topbar">
        <a className="brand" href="/members" aria-label="Phoenix Blog 成员目录首页">
          <span className="brand-mark">P</span>
          <span>Phoenix Blog</span>
        </a>
        <span className="source-note">数据源: {generatedBy}</span>
      </header>

      <main className="directory-main">
        <section className="page-heading" aria-labelledby="page-title">
          <div>
            <p className="kicker">成员数据集</p>
            <h1 id="page-title">团队成员目录</h1>
            <p>查看团队成员状态、角色与项目参与情况。</p>
          </div>
          <div className="dataset-stamp" aria-label={`初始 ${total} 条数据`}>
            <strong>{total}</strong>
            <span>条初始记录</span>
          </div>
        </section>

        <MemberCreator client:load initialTotal={total} />

        <section className="metrics" aria-label="数据概览">
          <div><span>成员总数</span><strong>{members.length}</strong></div>
          <div><span>当前在线</span><strong>{activeCount}</strong></div>
          <div><span>参与项目</span><strong>{projectCount}</strong></div>
          <div><span>覆盖城市</span><strong>{new Set(members.map((member) => member.city)).size}</strong></div>
        </section>

        <section className="directory-panel" aria-labelledby="directory-title">
          <div className="result-bar">
            <h2 id="directory-title">成员列表</h2>
            <span>显示前 <strong>{visibleMembers.length}</strong> 条，共 {total} 条记录</span>
          </div>
          <div className="table-wrap" id="member-table">
            <table>
              <thead>
                <tr>
                  <th>成员</th>
                  <th>角色</th>
                  <th>城市</th>
                  <th>状态</th>
                  <th>项目</th>
                  <th>加入日期</th>
                </tr>
              </thead>
              <tbody>
                {visibleMembers.map((member) => (
                  <tr key={member.id}>
                    <td data-label="成员" className="member-cell">
                      <span className="avatar" aria-hidden="true">{member.name.slice(0, 1)}</span>
                      <span><strong>{member.name}</strong><small>{member.email}</small></span>
                    </td>
                    <td data-label="角色">{member.role}</td>
                    <td data-label="城市">{member.city}</td>
                    <td data-label="状态"><Status status={member.status} minutes={member.lastActiveMinutes} /></td>
                    <td data-label="项目" className="number-cell">{member.projects}</td>
                    <td data-label="加入日期" className="date-cell">{member.joinedOn}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      </main>
    </div>
  );
}

function Status({ status, minutes }: { status: Member["status"]; minutes: number }) {
  const labels = { active: "在线", away: "暂离", offline: "离线" };
  const detail = status === "active"
    ? `${minutes % 60 + 1} 分钟前活跃`
    : status === "away" ? "稍后回来" : "当前不在线";
  return (
    <span className={`status status-${status}`} title={detail}>
      <span aria-hidden="true" />{labels[status]}
    </span>
  );
}
