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

## 3. 当前写法

后端声明输入 DTO、公开 Resource，并在路由上绑定二者：

```rust
#[phoenix::contract(input)]
#[derive(Deserialize)]
pub struct StoreMemberInput {
    pub name: String,
}

#[phoenix::contract(resource, name = "Member")]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberResource {
    pub id: u32,
    pub name: String,
    pub joined_on: String,
}

Routes::new()
    .post("/api/members", typed(MemberController::store))
    .name("members.store")
    .action::<StoreMemberInput, MemberResource>()
```

Vite 自动生成 action 和类型，React 不传路由字符串、URL或泛型：

```tsx
import { members } from "../generated/routes.js";

const member = await members.store({ name });
```

页面 Props 使用 `#[phoenix::contract(page, page = "...")]`，共享 Props 使用
`#[phoenix::contract(shared)]`。生成产物位于 `views/generated/contracts.ts`，并包含
`PhoenixPageProps`、`PhoenixSharedProps` 和稳定内容 hash。

## 4. 自动生成流程

```text
Rust Request / Props / Resource
  -> #[phoenix::contract(...)] 标记方向和公开名称
  -> phoenix-vite 读取 Rust 类型与 Serde wire 规则
  -> 生成 TypeScript 类型、页面映射、action 和 contract hash
  -> React / tsc / SSR renderer 使用同一产物
```

设计约束：

- 开发者不手动执行类型生成命令。Vite 启动、构建和 Rust 文件热更新会刷新契约。
- 生成文件位于 `views/generated/` 或 Vite 虚拟模块中，标记为只读且不进入 Git。
- CI 在临时目录重新生成并执行 TypeScript 类型检查，避免本地陈旧文件掩盖错误。
- 当前产物记录契约内容 hash；client manifest、renderer manifest 和常驻 renderer 握手会校验同一个 hash，不一致时启动或渲染失败。
- 当前支持命名 struct、unit enum、嵌套契约、`Option`、`Vec`、字符串键 Map，以及 `rename`、`rename_all`、`default`、`flatten`、`alias`、方向性 `skip` 和 `skip_serializing_if`。输入 alias 用于兼容性与冲突检查，生成的新请求统一使用规范字段名。
- 不支持的数据 enum、tuple/generic struct、不安全大整数，以及会改变 wire 形态但尚不能准确表达的 Serde 属性会明确中止构建，不会退化成 `any` 或猜测类型。

Rust 属性宏不向源码目录写文件；实际导出由 Vite 受控构建阶段完成。

## 5. 字段映射

| Rust | TypeScript | 说明 |
| --- | --- | --- |
| `String`, `&str` | `string` | 输入契约通常拥有数据，不导出引用语义 |
| 整数与浮点数 | `number` 或明确的大整数表示 | 超过 JS 安全整数范围时禁止无提示转成 `number` |
| `bool` | `boolean` | 处理 HTML checkbox 缺失值语义 |
| `Option<T>` | 可选或 `T \| null` | 必须区分字段缺失和显式 null，由属性决定 |
| `Vec<T>` | `T[]` | 支持嵌套字段错误路径 |
| Rust unit enum | 字符串联合 | 遵守方向性的 Serde rename、rename_all、alias 与 skip 设置 |
| `Id<T>` | 品牌化 string/number | 避免不同模型 ID 被误传 |
| `DateTime` | ISO 8601 string | 客户端不自动假设本地时区 |
| `Secret<T>` | 底层输入类型 | 自动附加敏感元数据，禁止进入输出契约 |

Wire name 以 Serde 序列化/反序列化后的最终名称为准。契约导出器不能另造一套与 Serde 不一致的命名规则。

## 6. 同名与冲突规则

### 类型同名

契约身份当前使用 `namespace + name + direction`，不使用无法区分方向的裸 struct 名。例如：

```text
auth.LoginInput
admin.LoginInput
```

两者可以共存，并生成到同一个只读模块的 TypeScript namespace：

```ts
import type { auth, admin } from "../generated/contracts.js";

type Login = auth.LoginInput;
type AdminLogin = admin.LoginInput;
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

当前 `Validate` 始终在服务端运行，并返回稳定字段错误。把必填、长度、数字范围等**验证规则**生成到 React 仍未实现；前端不能因为本地检查通过就跳过服务端验证。

首版已生成 input 契约的运行时字段表（`XxxFields`：wire name / 类型标签 / required），供 `form.field("...")` 绑定控件属性。完整客户端规则同步见后续切片；约定见 [REACT_DX_FORMS.md](REACT_DX_FORMS.md)。

以下规则只在服务端运行：数据库唯一性、权限、限流、密码校验、外部服务查询和任意异步自定义规则。React 必须正常展示服务端返回的字段错误，不能因为客户端规则通过就假设请求有效。

## 8. 兼容性与版本

- 删除必填字段、改变 wire type、收窄枚举或更改 null 语义属于破坏性变更。
- 新增可选字段通常兼容；新增必填字段必须提供默认值或升级版本。
- 生成的 TypeScript 模块携带 `contractHash`。
- 页面协议、client/renderer manifest 和 renderer worker 握手使用同一个 `contract_hash`；生产资源或 renderer 与当前 Rust 契约不一致时失败关闭。
- CSP nonce 是每请求文档执行上下文，只通过 `ResponseContext` 和 renderer v2 顶层字段传递；它不进入 `PageEnvelope`、生成 TypeScript、共享 props 或 `contract_hash`。

## 9. 验收标准

- Rust 中只定义一次 action 输入和 Resource 字段，React 能完成类型安全提交。
- 修改 Rust 字段名后，React 旧代码在类型检查中失败。
- 两个模块可以拥有同名 `LoginInput`，且导入路径无歧义。
- 同一命名空间或最终 wire name 碰撞时构建失败并给出来源位置。
- 输入契约不会自动成为输出契约；敏感字段仍必须使用独立输入 DTO，并通过 Resource 白名单避免输出。
- SPA、SSR 和 Islands 对同一契约产生一致 JSON。
