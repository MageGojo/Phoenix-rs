import { island } from "@phoenix/react";

import MemberDirectory, { type Member } from "../../islands/member-directory.js";

const MemberDirectoryIsland = island(MemberDirectory);

export interface MembersIndexProps {
  members: Member[];
  generatedBy: string;
  total: number;
}

export default function MembersIndex({ members, generatedBy, total }: MembersIndexProps) {
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

        <MemberDirectoryIsland
          initialMembers={members}
          initialTotal={total}
        />
      </main>
    </div>
  );
}
