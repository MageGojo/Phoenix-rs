import { useState } from "react";

import { FieldError, Form } from "@apizero/react";

import { members } from "../generated/routes.js";
import { StoreMemberInputFields } from "../generated/contracts.js";
import type { Member } from "../generated/contracts.js";

export interface MemberCreatorProps {
  initialTotal: number;
}

export default function MemberCreator({ initialTotal }: MemberCreatorProps) {
  const [createdMembers, setCreatedMembers] = useState<Member[]>([]);
  const [feedback, setFeedback] = useState<{
    type: "success" | "error";
    message: string;
  } | null>(null);

  return (
    <section className="member-creator" aria-label="新增成员">
      <Form
        className="member-composer"
        action={members.store}
        initialValues={{ name: "" }}
        fields={StoreMemberInputFields}
        onSuccess={(member) => {
          setCreatedMembers((current) => [member, ...current]);
          setFeedback({ type: "success", message: `Rust 已创建 ${member.name}` });
        }}
        onError={(error) => {
          setFeedback({
            type: "error",
            message: error instanceof Error ? error.message : "提交失败，请重试。",
          });
        }}
      >
        {(form) => (
          <>
            <div>
              <strong>新增成员</strong>
              <span>当前共 {initialTotal + createdMembers.length} 条记录</span>
            </div>
            <label htmlFor="new-member-name">
              <span>成员姓名</span>
              <input
                {...form.field("name")}
                id="new-member-name"
                placeholder="输入姓名"
                autoComplete="off"
                disabled={form.processing}
              />
            </label>
            <FieldError errors={form.errors} name="name" className="member-feedback member-feedback-error" />
            <button type="submit" disabled={!form.data.name.trim() || form.processing}>
              {form.processing ? "提交中..." : "添加成员"}
            </button>
            <p
              className={`member-feedback${feedback?.type === "error" ? " member-feedback-error" : ""}`}
              aria-live="polite"
              role={feedback?.type === "error" ? "alert" : undefined}
            >
              {feedback?.message ?? ""}
            </p>
          </>
        )}
      </Form>

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
