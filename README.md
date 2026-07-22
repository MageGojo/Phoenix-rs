# Phoenix

Phoenix 是一个处于规划阶段、以 Hyper 为 HTTP 核心的 Rust 网站应用框架。项目目标是在 Rust 的类型安全与性能基础上，提供接近 Laravel 的开发体验，并默认集成 React + TypeScript 视图。

> `Phoenix` 目前是工作名称，与 Elixir 生态中的 Phoenix Framework 存在重名风险。正式发布前必须完成命名与 crate 可用性评估。

## 目标体验

开发者负责路由、控制器、模型和 `views/` 下的 `.tsx` / `.jsx` 文件。框架负责请求解析、验证、数据库访问、React 页面响应、错误处理和安全默认值。

下面是目标 API 草案，不代表当前已有实现：

```rust
pub async fn show(user: User) -> Result<Response> {
    render("users/show", props! {
        "user" => user,
    })
}
```

```tsx
type Props = {
  user: User;
};

export default function Show({ user }: Props) {
  return <h1>{user.name}</h1>;
}
```

## 当前状态

当前仓库只包含产品规划、技术方案和空目录骨架，尚未包含可运行代码。

- [产品需求](docs/PRODUCT.md)
- [架构设计](docs/PROJECT.md)
- [开发者体验草案](docs/DX.md)
- [安全与数据传输](docs/SECURITY.md)
- [技术决策](docs/DECISIONS.md)
- [当前进度](docs/PROGRESS.md)
- [下一阶段](docs/NEXT.md)

## 计划中的仓库结构

```text
crates/                 Rust 框架组件
packages/phoenix-react/ React 客户端适配层
examples/blog/          贯穿开发过程的参考应用
docs/                   产品、架构与项目记录
```

## 第一版边界

第一版聚焦常规服务端网站应用：控制器、路由、请求、验证、Toasty 模型与迁移、React 页面响应、中间件、会话、CSRF、错误处理和测试工具。CLI 代码生成、管理后台、队列、邮件、WebSocket、SSR 与插件市场不进入首个可用版本。
