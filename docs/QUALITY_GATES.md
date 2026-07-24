# 工程质量门禁

Phoenix 的发布门禁分为常规回归、安全供应链、覆盖率、性能基线和 fuzz。所有 CI 使用已提交锁文件；托管任务只使用合成测试凭据，不读取生产 secret。

## 工作流

| 工作流 | 触发 | 阻断条件 |
| --- | --- | --- |
| `CI` | `main` push、PR、手动 | Rustfmt、严格 Clippy、workspace tests、Node tests/typecheck/build、PostgreSQL contract 或 CSP E2E 失败 |
| `Security` | `main` push、PR、每周、手动 | Git 历史发现 secret、Rust advisory/license/source policy 或高危 npm advisory 失败 |
| `Coverage` | `main` push、PR、手动 | 任一测试失败或行覆盖率低于当前最低线 |
| `Benchmarks` | 每周、手动 | Criterion benchmark 构建或执行失败；估算结果保存 30 天 |
| `Fuzz` | 每周、手动 | HTTP 边界或盲索引 envelope 在 60 秒 sanitizer 运行中崩溃 |

覆盖率最低线是 Rust workspace 85%、`@apizero/react` 85%、`@apizero/react-ssr` 50%、`@apizero/vite` 75%、博客示例 25%。这些值低于首次本地基线，后续提高时必须先补测试，不通过删除文件或排除业务模块制造虚假增长。

## 本地复现

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm ci
npm run ci:node
npm run test:e2e:ssr-csp
cargo deny check advisories licenses bans sources
npm audit --audit-level=high
cargo llvm-cov --workspace --locked --all-features --fail-under-lines 85
npm run test:coverage:js
cargo bench --locked --manifest-path benchmarks/Cargo.toml --bench blind_index
cd fuzz && cargo metadata --locked --format-version 1 > /dev/null && cargo +nightly fuzz build
```

PostgreSQL contract 必须连接一次性测试库：

```bash
PHOENIX_TEST_POSTGRES_URL='postgresql://TEST_USER:TEST_PASSWORD@127.0.0.1:5432/phoenix_test' \
  cargo test --locked -p phoenix-database --test toasty_integration \
  postgresql_crud_relations_and_pagination_when_configured -- --exact
```

## 依赖策略

`deny.toml` 默认拒绝未知 registry、未知 Git source、yanked crate 和未允许许可证。workspace 内部 path dependency 在技术预览阶段只告警；发布 crate 前必须增加明确版本。重复版本暂时告警，并通过审计输出持续收敛。

JWT 使用 `jsonwebtoken/aws_lc_rs`，因此依赖图不再包含受 RUSTSEC-2023-0071 影响的 `rsa`。`RUSTSEC-2025-0134` 是唯一临时 advisory 例外：Phoenix TLS PEM 入口和 Toasty 0.8 PostgreSQL driver 仍依赖 `rustls-pemfile`。删除条件同时包括：

1. Phoenix 直接 PEM 读取迁移到 `rustls-pki-types::pem`。
2. Toasty 发布不再传递 `rustls-pemfile` 的版本并完成升级。
3. `cargo tree -i rustls-pemfile` 不再返回依赖路径。

满足条件后必须从 `deny.toml` 删除该编号；不得扩大到 advisory 类别级忽略。

## Secret 与产物边界

Gitleaks 使用默认规则扫描完整 Git 历史且输出脱敏。未跟踪的 IDE 状态、临时代理配置、coverage、Criterion 输出、fuzz corpus/artifacts 和构建目录由精确 `.gitignore` 规则隔离。真实 URL、凭据、转储、抓包和运行日志不进入 fixture 或 CI artifact。

Criterion 数字只在同一 runner 类型和工具链内比较。AWS-LC 构建下首次本地盲索引基线为 360.89-364.79 ns/次；GitHub 定时任务产生独立 Linux 基线，不能把两种机器的绝对耗时直接比较。
