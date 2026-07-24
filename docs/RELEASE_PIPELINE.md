# 发版流水线（Release Pipeline）

Phoenix-rs 提供 **打版本包 →（上传）→ 校验 → 迁移 → 原子切换 → 可回滚** 的约定与 CLI。  
框架管制品格式与本机/服务器上的安装状态机；开发者管机器、密钥与进程管理（systemd 等）。

## 命令

| 命令 | 作用 |
| --- | --- |
| `px release` | 在应用源码根构建 release 制品（Rust + Vite） |
| `px release:install` | 在部署根校验、解压、迁移、切换 `current` |
| `px release:rollback` | 切回上一版（默认不自动 DB rollback） |
| `px release:status` | 查看 current / previous / releases |

## 端到端

```bash
# CI / 开发机
px release --version 0.1.2 --tarball
scp dist/releases/0.1.2/*.tar.gz user@prod:/tmp/

# 服务器（首次准备 shared/.env 与 storage）
export PHOENIX_DEPLOY_ROOT=/var/www/my-app
px release:install --tarball /tmp/my-app-0.1.2-*.tar.gz --restart-cmd 'systemctl restart my-app'
px release:status

# 出问题
px release:rollback --steps 1 --restart-cmd 'systemctl restart my-app'
```

上传本身可用 `scp` / `rsync` / 对象存储；**不**内置多机编排。

## 部署目录

```text
$PHOENIX_DEPLOY_ROOT/
  releases/<ver>/     # 不可变版本：bin/、public/、config/、manifest.toml
  current -> releases/<ver>
  shared/             # .env、storage/（密钥与持久化，不进制品）
  tmp/release.lock
  tmp/previous -> …
  deploy/restart.sh   # 可选
```

`current` 通过原子 symlink 切换。`.env` 与 `storage` 从 `shared/` 链入各 release。

## 制品内容

- `bin/<app>`、`bin/phoenix-manage`（迁移用，目标机可不装完整源码树）
- `public/assets/`、`public/ssr/`（Vite 产物；**不要**把 `assets/` 内容摊平进 `public/`）
- 应用启动后由 `ServeProductionAssets` 按 `phoenix-manifest.json` 白名单对外提供 `/assets/*`；页面须 `Page::production_assets(..., "client")` 写入 hashed URL，勿硬编码 `/assets/phoenix.js`
- `config/*.toml`（非密钥）
- `database/migrations/`
- `manifest.toml`（版本、checksum、contract_hash）

不含：`.env`、`node_modules`、`target`、密钥文件。

## 失败与回滚

- 校验 / 迁移失败：**不切换** `current`
- 重启失败：代码可能已切换 → 人工 `release:rollback`
- 默认代码回滚 **不** 跑 `migrate down`（生产默认 forward-only）

## API

- Crate：`phoenix-release`（布局、manifest、pack、install、rollback）
- CLI：`px` 子命令（见上）
- 决策：`DECISIONS.md` ADR-040
