# Rust 与 TypeScript 数据契约

## 1. 目标

Rust 是请求字段、页面 Props 和公开 Resource 的唯一数据契约源。开发者在后端声明一次字段，React 直接导入自动生成的 TypeScript 类型与运行时表单描述，不再手写一份重复接口。

这套能力不仅是“把 struct 翻译成 interface”，还必须保证：

- Rust 反序列化名称与浏览器提交字段完全一致。
- React 表单字段有自动补全，拼错字段名会在类型检查中失败。
- 可转换的验证规则同步到客户端，用于即时反馈；服务端始终是最终验证者。
- 字段、类型和命名空间冲突在构建阶段失败，不能静默覆盖。
- 密码等输入字段可以出现在输入契约中，但不会因此进入日志、旧输入或页面输出。
- SPA、SSR 与 Islands 使用相同契约和相同序列化结果。

## 2. 契约种类

| 契约 | 数据方向 | Rust 来源 | React 用途 |
| --- | --- | --- | --- |
| Input | 浏览器到服务端 | Request DTO | 表单字段、请求 body、客户端验证提示 |
| Page Props | 服务端到页面 | Props struct | 页面组件参数 |
| Resource | 服务端到浏览器 | 显式 Resource/ViewModel | 可复用公开业务数据 |
| Shared Props | 服务端到所有页面 | Shared props struct | 当前用户、flash、CSRF 等公共数据 |

数据库模型不自动成为前端契约。模型必须先转换为 Resource，防止新增数据库字段时意外暴露给浏览器。

## 3. 目标写法

后端只声明一次登录请求：

```rust
#[derive(FromRequest, Validate, Contract)]
#[contract(namespace = "auth", name = "LoginInput", direction = "input")]
pub struct LoginRequest {
    #[validate(length(min = 3, max = 120))]
    pub user: String,

    #[sensitive]
    #[validate(length(min = 8, max = 128))]
    pub password: Secret<String>,
}
```

自动生成的产物在概念上包含类型与运行时描述：

```ts
export type LoginInput = {
  user: string;
  password: string;
};

export const LoginInputContract: InputContract<LoginInput>;
```

React 端直接使用，不再重写 `user` 和 `password`：

```tsx
import {
  LoginInputContract,
} from "#phoenix/contracts/auth";
import { useForm } from "@phoenix/react";

export default function Login() {
  const form = useForm(LoginInputContract);

  return (
    <form onSubmit={form.submit("auth.login")}>
      <input {...form.field("user")} autoComplete="username" />
      <input
        {...form.field("password")}
        type="password"
        autoComplete="current-password"
      />
      <button type="submit">Login</button>
    </form>
  );
}
```

`useForm()` 从运行时契约推导完整 TypeScript 类型，不要求再传泛型。`form.field()` 只接受契约中的字段名，并负责 `name`、当前值、变更事件和字段错误。能无歧义推导的字段获得安全空值；其余字段要求调用者提供初值。页面布局、控件类型和文案仍由开发者决定，框架不根据字段自动生成低质量 UI。

## 4. 自动生成流程

```text
Rust Request / Props / Resource
  -> #[derive(Contract)] 生成 schema 实现并注册
  -> Phoenix contract exporter 汇总并校验
  -> 版本化 contract manifest
  -> phoenix-vite 生成 TypeScript 类型与运行时描述
  -> React / tsc / SSR renderer 使用同一产物
```

命名路由已经先行使用同一生成目录：`phoenix-vite` 从 Rust 的字面量 `.name("members.store")` 生成 `members.store` TypeScript 常量，提供属性补全与重命名检查。这个常量只描述路由身份，不代表输入/输出契约已经完成；当前 `Request -> Response` 处理器仍无法自动推导 action body 与返回 Resource。

设计约束：

- 开发者不手动执行类型生成命令。官方开发流程在 Rust 编译成功后自动刷新契约。
- 生成文件位于 `views/generated/` 或 Vite 虚拟模块中，标记为只读且不进入 Git。
- CI 在临时目录重新生成并执行 TypeScript 类型检查，避免本地陈旧文件掩盖错误。
- 生产构建记录契约 manifest 的内容哈希；Rust 服务、浏览器资源和 SSR renderer 的哈希不一致时构建失败或拒绝启动。
- 具体采用现有类型导出库还是自研 derive，需要通过递归类型、泛型、枚举、日期、ID、新类型和 Serde 属性的 spike 决定。

Rust 过程宏不应直接向源码目录写文件。宏只生成 schema 与注册代码，实际导出由受控构建阶段完成。

## 5. 字段映射

| Rust | TypeScript | 说明 |
| --- | --- | --- |
| `String`, `&str` | `string` | 输入契约通常拥有数据，不导出引用语义 |
| 整数与浮点数 | `number` 或明确的大整数表示 | 超过 JS 安全整数范围时禁止无提示转成 `number` |
| `bool` | `boolean` | 处理 HTML checkbox 缺失值语义 |
| `Option<T>` | 可选或 `T \| null` | 必须区分字段缺失和显式 null，由属性决定 |
| `Vec<T>` | `T[]` | 支持嵌套字段错误路径 |
| Rust enum | 字符串联合或 tagged union | 遵守 Serde tag/content/rename 设置 |
| `Id<T>` | 品牌化 string/number | 避免不同模型 ID 被误传 |
| `DateTime` | ISO 8601 string | 客户端不自动假设本地时区 |
| `Secret<T>` | 底层输入类型 | 自动附加敏感元数据，禁止进入输出契约 |

Wire name 以 Serde 序列化/反序列化后的最终名称为准。契约导出器不能另造一套与 Serde 不一致的命名规则。

## 6. 同名与冲突规则

### 类型同名

契约身份使用 `namespace + name + version`，不使用裸 struct 名。例如：

```text
auth.LoginInput@1
admin.LoginInput@1
```

两者可以共存，并生成到不同模块：

```ts
import type { LoginInput } from "#phoenix/contracts/auth";
import type { LoginInput as AdminLoginInput } from "#phoenix/contracts/admin";
```

同一命名空间内的同名契约直接构建失败。生成器不追加数字，不按注册顺序覆盖。

### 字段同名

以下情况都必须在 derive 或导出阶段失败，并指出两个 Rust 字段的位置：

- 两个字段经过 `rename_all` 后得到相同 wire name。
- `#[serde(rename = "...")]` 与另一个字段冲突。
- `flatten` 展开后字段冲突。
- camelCase、snake_case 或大小写归一化导致碰撞。
- 字段 alias 与正式字段名冲突，导致反序列化存在歧义。

不同契约中的同名字段是正常情况，因为它们处于各自类型作用域。

### 页面 Props 与 Input 同名

输入与输出是不同方向的契约，即使字段相同也不自动合并。这样 `LoginInput.password` 不可能因为复用类型而进入页面 Props。确需共享时，应提取不含敏感数据的 Resource。

## 7. 验证同步

可以安全转换的规则包括必填、长度、数字范围、枚举、简单格式和确认字段。它们同时生成 TypeScript 元数据和合适的 HTML 属性。

以下规则只在服务端运行：数据库唯一性、权限、限流、密码校验、外部服务查询和任意异步自定义规则。React 必须正常展示服务端返回的字段错误，不能因为客户端规则通过就假设请求有效。

## 8. 兼容性与版本

- 删除必填字段、改变 wire type、收窄枚举或更改 null 语义属于破坏性变更。
- 新增可选字段通常兼容；新增必填字段必须提供默认值或升级版本。
- 页面协议携带 `contract_version` 和 `contract_hash`。
- SPA 导航发现 hash 不一致时执行完整刷新。
- SSR/Islands renderer 与 Rust 服务 hash 不一致时拒绝渲染，避免生成错误 HTML。

## 9. 验收标准

- Rust 中只定义一次 `user`、`password`，React 能完成类型安全提交。
- 修改 Rust 字段名后，React 旧代码在类型检查中失败。
- 两个模块可以拥有同名 `LoginInput`，且导入路径无歧义。
- 同一命名空间或最终 wire name 碰撞时构建失败并给出来源位置。
- `password` 的用户输入值不出现在旧输入、日志、页面 Props、SSR HTML 或 hydration 数据中；字段名和空表单控件可以正常出现。
- SPA、SSR 和 Islands 对同一契约产生一致 JSON。
