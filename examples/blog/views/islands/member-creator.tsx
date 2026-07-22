import { type FormEvent, useState } from "react";
import { callRust } from "@phoenix/react";

import type { Member } from "../types/member.js";

export interface MemberCreatorProps {
  initialTotal: number;
}

export default function MemberCreator({ initialTotal }: MemberCreatorProps) {
  const [draftName, setDraftName] = useState("");
  const [createdMembers, setCreatedMembers] = useState<Member[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [feedback, setFeedback] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  async function addMember(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const name = draftName.trim();
    if (!name || submitting) return;

    setSubmitting(true);
    setFeedback(null);
    try {
      const member = await callRust<Member>("members.store", { name });
      setCreatedMembers((current) => [member, ...current]);
      setDraftName("");
      setFeedback({ type: "success", message: `Rust 已创建 ${member.name}` });
    } catch (error) {
      setFeedback({
        type: "error",
        message: error instanceof Error ? error.message : "提交失败，请重试。",
      });
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <section className="member-creator" aria-label="新增成员">
      <form className="member-composer" onSubmit={addMember}>
        <div>
          <strong>新增成员</strong>
          <span>当前共 {initialTotal + createdMembers.length} 条记录</span>
        </div>
        <label htmlFor="new-member-name">
          <span>成员姓名</span>
          <input
            id="new-member-name"
            value={draftName}
            onChange={(event) => setDraftName(event.target.value)}
            placeholder="输入姓名"
            autoComplete="off"
            disabled={submitting}
          />
        </label>
        <button type="submit" disabled={!draftName.trim() || submitting}>
          {submitting ? "提交中..." : "添加成员"}
        </button>
        <p
          className={`member-feedback${feedback?.type === "error" ? " member-feedback-error" : ""}`}
          aria-live="polite"
          role={feedback?.type === "error" ? "alert" : undefined}
        >
          {feedback?.message ?? ""}
        </p>
      </form>

      {createdMembers.length > 0 && (
        <div className="created-members" aria-live="polite">
          <h2>本次新增</h2>
          {createdMembers.map((member) => (
            <div className="created-member" key={member.id}>
              <span className="avatar" aria-hidden="true">{member.name.slice(0, 1)}</span>
              <span>
                <strong>{member.name}</strong>
                <small>{member.email}</small>
              </span>
              <span>{member.city}</span>
              <span>{member.role}</span>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
