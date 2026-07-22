import { useEffect, useMemo, useState } from "react";

interface Member {
  id: number;
  name: string;
  email: string;
  city: string;
  role: string;
  status: "active" | "away" | "offline";
  projects: number;
  joinedOn: string;
  lastActiveMinutes: number;
}

interface MembersIndexProps {
  members: Member[];
  generatedBy: string;
  total: number;
}

type SortKey = "name" | "projects" | "joinedOn";

const PAGE_SIZE = 10;

export default function MembersIndex({ members, generatedBy, total }: MembersIndexProps) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("all");
  const [role, setRole] = useState("all");
  const [sortKey, setSortKey] = useState<SortKey>("name");
  const [sortDirection, setSortDirection] = useState<"ascending" | "descending">("ascending");
  const [page, setPage] = useState(1);

  useEffect(() => {
    document.title = "团队成员目录 | Phoenix Blog";
  }, []);

  const roles = useMemo(
    () => Array.from(new Set(members.map((member) => member.role))).sort(),
    [members],
  );

  const filtered = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase("zh-CN");
    return members
      .filter((member) => {
        const searchable = `${member.name} ${member.email} ${member.city} ${member.role}`
          .toLocaleLowerCase("zh-CN");
        return (
          (!normalizedQuery || searchable.includes(normalizedQuery)) &&
          (status === "all" || member.status === status) &&
          (role === "all" || member.role === role)
        );
      })
      .sort((left, right) => {
        const direction = sortDirection === "ascending" ? 1 : -1;
        if (sortKey === "projects") {
          return (left.projects - right.projects) * direction;
        }
        return left[sortKey].localeCompare(right[sortKey], "zh-CN") * direction;
      });
  }, [members, query, role, sortDirection, sortKey, status]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const safePage = Math.min(page, pageCount);
  const visibleMembers = filtered.slice((safePage - 1) * PAGE_SIZE, safePage * PAGE_SIZE);
  const activeCount = members.filter((member) => member.status === "active").length;
  const projectCount = members.reduce((sum, member) => sum + member.projects, 0);

  function updateFilter(update: () => void) {
    update();
    setPage(1);
  }

  function toggleSort(nextKey: SortKey) {
    if (sortKey === nextKey) {
      setSortDirection((current) => current === "ascending" ? "descending" : "ascending");
    } else {
      setSortKey(nextKey);
      setSortDirection("ascending");
    }
    setPage(1);
  }

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
            <p>由 Rust 后端生成 100 条假数据，React 负责筛选、排序与分页。</p>
          </div>
          <div className="dataset-stamp" aria-label={`共 ${total} 条数据`}>
            <strong>{total}</strong>
            <span>条记录</span>
          </div>
        </section>

        <section className="metrics" aria-label="数据概览">
          <div><span>成员总数</span><strong>{total}</strong></div>
          <div><span>当前在线</span><strong>{activeCount}</strong></div>
          <div><span>参与项目</span><strong>{projectCount}</strong></div>
          <div><span>覆盖城市</span><strong>{new Set(members.map((member) => member.city)).size}</strong></div>
        </section>

        <section className="directory-panel" aria-label="成员筛选与列表">
          <div className="toolbar">
            <label className="search-field">
              <span>搜索成员</span>
              <input
                type="search"
                value={query}
                placeholder="姓名、邮箱、城市或角色"
                onChange={(event) => updateFilter(() => setQuery(event.target.value))}
              />
            </label>
            <label>
              <span>状态</span>
              <select value={status} onChange={(event) => updateFilter(() => setStatus(event.target.value))}>
                <option value="all">全部状态</option>
                <option value="active">在线</option>
                <option value="away">暂离</option>
                <option value="offline">离线</option>
              </select>
            </label>
            <label>
              <span>角色</span>
              <select value={role} onChange={(event) => updateFilter(() => setRole(event.target.value))}>
                <option value="all">全部角色</option>
                {roles.map((item) => <option key={item} value={item}>{item}</option>)}
              </select>
            </label>
          </div>

          <div className="result-bar" aria-live="polite">
            <span>找到 <strong>{filtered.length}</strong> 条记录</span>
            {(query || status !== "all" || role !== "all") && (
              <button
                type="button"
                className="text-button"
                onClick={() => {
                  setQuery("");
                  setStatus("all");
                  setRole("all");
                  setPage(1);
                }}
              >清除筛选</button>
            )}
          </div>

          {visibleMembers.length > 0 ? (
            <div className="table-wrap" id="member-table">
              <table>
                <thead>
                  <tr>
                    <SortableHeader label="成员" column="name" active={sortKey} direction={sortDirection} onSort={toggleSort} />
                    <th>角色</th>
                    <th>城市</th>
                    <th>状态</th>
                    <SortableHeader label="项目" column="projects" active={sortKey} direction={sortDirection} onSort={toggleSort} />
                    <SortableHeader label="加入日期" column="joinedOn" active={sortKey} direction={sortDirection} onSort={toggleSort} />
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
          ) : (
            <div className="empty-state">
              <strong>没有匹配的成员</strong>
              <p>尝试缩短搜索词，或清除状态与角色筛选。</p>
            </div>
          )}

          <nav className="pagination" aria-label="成员列表分页">
            <span>第 {safePage} / {pageCount} 页</span>
            <div>
              <button type="button" disabled={safePage === 1} onClick={() => setPage((value) => value - 1)}>上一页</button>
              <button type="button" disabled={safePage === pageCount} onClick={() => setPage((value) => value + 1)}>下一页</button>
            </div>
          </nav>
        </section>
      </main>
    </div>
  );
}

function SortableHeader({
  label,
  column,
  active,
  direction,
  onSort,
}: {
  label: string;
  column: SortKey;
  active: SortKey;
  direction: "ascending" | "descending";
  onSort: (column: SortKey) => void;
}) {
  const selected = active === column;
  return (
    <th aria-sort={selected ? direction : "none"}>
      <button type="button" className="sort-button" onClick={() => onSort(column)}>
        {label}<span aria-hidden="true">{selected ? (direction === "ascending" ? " ↑" : " ↓") : " ↕"}</span>
      </button>
    </th>
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
