# 聚合支付（phoenix-pay）

## 目标

提供 Laravel Cashier 风格的统一支付抽象：金额恒为**整数分**、Provider 可插拔、订单状态机严格校验、异步通知按 `provider + out_trade_no` 幂等处理，并通过 `PayFeature`（`phoenix-plugin` 机制）一次性装配回调路由与 `payments` 迁移。

内置**完整可用的 `MockProvider`**（测试 / 本地开发），以及**已落地的真实网关**：微信支付 APIv3（Native 扫码）与支付宝当面付（RSA2）——下单 / 查询 / 关单 / 回调验签与解密均已实现，HTTP 走可插拔的 `PayHttp` 传输层（默认 `HyperPayHttp`：hyper 1.x + rustls + 系统根证书）。**先验签后取数**是硬约束：任何未通过签名验证的应答 / 回调都不会产出 `NotifyEvent`。

## 金额整数分（铁律）

- `Amount { minor: u64, currency: Currency }`，CNY 的 minor 单位是**分**。
- 全程整数运算：构造（`Amount::cny(1234)`、`Amount::cny_yuan_fen(12, 34)`）、加法（`checked_add`，溢出/币种不一致报错）、展示（`Display` 输出 `12.34 CNY`）**均无浮点**。
- 千万不要在业务代码里出现 `f32`/`f64` 金额，序列化也走 `{ "minor": 1234, "currency": "CNY" }`。

## 公开 API

| 类型 | 职责 |
| --- | --- |
| `Amount` / `Currency` | 整数分金额；首版仅 `CNY` |
| `PaymentStatus` | 订单状态机：`Created → Pending → Paid / Failed / Closed`，`Paid ⇄ Refunding → Refunded`；非法迁移返回 `PayError::InvalidTransition` |
| `PaymentProvider` | `key()` / `create(&CreateOrder)` / `verify_notify(&NotifyRequest)` / `query(out_trade_no)`，以及默认 `NotImplemented` 的 `close` / `refund` / `query_refund` / `download_bill`；异步风格与 `MailTransport` 一致（`BoxFuture`，可 `Arc<dyn>` 持有） |
| `RefundOrder` / `RefundStatus` / `RefundReceipt` / `RefundRecord` | 退款请求（`full` / `partial` / `reason`）、退款状态机、网关回执、持久化行 |
| `RefundNotifyEvent` / `RefundNotifyOutcome` | 已验签的退款异步通知，及其幂等处理结果 |
| `Bill` / `BillEntry` / `Discrepancy` / `Reconciliation` | 对账：账单、账单行、差异、比对结果（`reconcile` / `parse_bill_csv` / `parse_bill_csv_bytes`） |
| `PayHttp` / `HyperPayHttp` | 网关 HTTP 传输接缝：`request(GatewayRequest) -> GatewayResponse`；默认实现 hyper 1.x + rustls（`rustls-native-certs` 系统根证书），测试可指向 127.0.0.1 假网关 |
| `PaymentAction` | `QrCode(String)` / `Redirect(String)` / `SdkParams(Value)`（`#[non_exhaustive]`） |
| `PaymentIntent` / `NotifyEvent` / `PaymentRecord` | 下单结果 / 已验签的规范化通知 / 存储行 |
| `PaymentStore` | 订单：`insert` / `find` / `transition` / `paid_within`；退款：`insert_refund` / `find_refund` / `refunds_for` / `transition_refund` / `record_refund_id`。内置 `MemoryPaymentStore` 与 `DbPaymentStore` |
| `PayManager` | 门面：`create` / `handle_notify`（幂等）/ `query` / `close` / `find_order` / `refund`（幂等）/ `handle_refund_notify`（幂等）/ `sync_refund` / `refunds_for` / `reconcile_day` / `reconcile_bill` |
| `PayFeature` | Plugin：回调路由 + `payments` / `payment_refunds` 迁移 |
| `Secret` | 可反序列化的密钥字段，`Debug` 输出 `[REDACTED]`，drop 时 zeroize |
| `PayError` | thiserror 稳定错误集：`Config`（密钥/配置问题 → 500）、`Gateway`（网关传输/应答验签失败 → 502）、`InvalidNotify`（回调验签/解析失败 → 400）、`NotImplemented`（→ 501）、`InvalidRefund` / `DuplicateRefund` / `RefundNotFound` / `RefundExceedsOrder` / `Reconcile` 等 |

## 状态机

订单状态机：

```text
Created ──> Pending ──> Paid ⇄ Refunding ──> Refunded
   │           │  └───> Failed
   └───────────┴──────> Closed
```

- 终态：`Failed` / `Closed` / `Refunded`（无出边）。
- 同状态重复迁移也算非法（幂等在 `PayManager` 层处理，不靠状态机放水）。
- `PayManager::create` 失败时会尽力把记录置为 `Closed`，保留审计行。
- **退款臂是双向的**：受理退款进 `Refunding`；成功退款累计够订单总额进 `Refunded`；**全部退款都失败则回到 `Paid`**——钱没动，订单确实还是已付。

退款有**自己的**状态机（一笔订单可以有多笔部分退款，各自独立）：

```text
Processing ──> Succeeded
     └───────> Failed
```

- `Processing` 是**成功受理**，不是失败：微信银行卡退款可能异步落地，用 `PayManager::sync_refund` 轮询。
- 终态不可逆：失败的退款要重试必须换一个新的 `out_refund_no`。

## Mock 用法（完整流程）

```rust
use std::sync::Arc;
use phoenix_pay::prelude::*;

let provider = MockProvider::new();
let manager = Arc::new(
    PayManager::builder()
        .provider(Arc::new(provider.clone()))
        .build(),               // 默认 MemoryPaymentStore
);

// 1. 下单：返回二维码文本
let intent = manager
    .create("mock", CreateOrder::new("T100", Amount::cny(1234), "会员月卡"))
    .await?;
assert!(matches!(intent.action, PaymentAction::QrCode(_)));

// 2. 模拟用户扫码付款，拿到平台会 POST 的通知体
let body = provider.mark_paid("T100")?;

// 3. 处理通知（或直接 POST 到 pay.notify.mock 路由）
let outcome = manager.handle_notify("mock", NotifyRequest::from_body(body)).await?;
assert!(matches!(outcome, NotifyOutcome::Processed(_)));

// 4. 重复通知幂等：返回 AlreadyProcessed，状态不再变化
```

## Feature 装配与路由

```rust
use phoenix::plugin::FeatureSet;

let features = FeatureSet::new().plugin(PayFeature::new(manager))?;
let parts = features.into_parts();   // routes / migrations
```

| 路由名（namespaced） | 方法与路径 | 说明 |
| --- | --- | --- |
| `pay.notify.wechat` | `POST /pay/notify/wechat` | 微信异步通知（验签 + 解密后入库；无签名 / 验签失败一律 400） |
| `pay.notify.alipay` | `POST /pay/notify/alipay` | 支付宝异步通知（RSA2 验签后入库；验签失败一律 400） |
| `pay.notify.mock` | `POST /pay/notify/mock` | Mock 通知（开发 / 测试） |
| `pay.notify.wechat.refund` | `POST /pay/notify/wechat/refund` | 微信**退款**异步通知（独立回调 URL） |
| `pay.notify.mock.refund` | `POST /pay/notify/mock/refund` | Mock 退款通知（开发 / 测试） |
| `pay.orders.show` | `GET /pay/orders/{provider}/{out_trade_no}` | 本地订单状态查询 |

**CSRF 说明**：回调由支付平台服务器调用，没有会话，也带不了 CSRF token。Phoenix 的 `Csrf` 中间件是按路由组显式挂载的（见 `docs/FEATURES.md` / 安全栈），因此把 `FeatureSet` 的支付路由**单独 merge、不要包进 Session/CSRF 中间件组**即可；通知的真实性由 `PaymentProvider::verify_notify` 的验签保证，而不是 CSRF。

## 存储

`PayFeature` 注册两条迁移：`202607260001 create payments table` 与 `202607260004 create payment_refunds table`。

```text
payments(id, provider, out_trade_no, amount, currency, status,
         subject, notify_payload, paid_at, created_at, updated_at)
UNIQUE (provider, out_trade_no)
INDEX  (provider, paid_at)

payment_refunds(id, provider, out_trade_no, out_refund_no, refund_id,
                amount, currency, status, reason, created_at)
UNIQUE (provider, out_refund_no)
INDEX  (provider, out_trade_no)
```

- `paid_at` 在订单**第一次**进入 `paid` 时打戳，之后不再改写——退款失败导致的 `Refunding → Paid` 不会把订单挪到另一个对账日。它是 `PaymentStore::paid_within` 的查询依据。
- 退款单独一张表：一笔订单可以有多笔部分退款，`(provider, out_refund_no)` 是幂等键，与订单的 `(provider, out_trade_no)` 同构。
- SQL 以 SQLite 优先（工作区默认），PostgreSQL 兼容；MySQL 的 `DROP INDEX IF EXISTS` 语法需调整。
- 两种实现：`MemoryPaymentStore`（测试 / 单机）与 `DbPaymentStore`（Toasty，重启存活）。应用的 `models!(...)` 需要同时登记 `PaymentRow` 与 `RefundRow`。

## 退款

```rust
// 全额退款
let receipt = manager
    .refund("wechat_native", RefundOrder::full("T-1001", "R-1001", Amount::cny(1234)))
    .await?;

// 部分退款：退多少 + 原订单总额（两个网关都要求后者）
let receipt = manager
    .refund(
        "alipay_f2f",
        RefundOrder::partial("T-1001", "R-1002", Amount::cny(300), Amount::cny(1234))
            .reason("尺码不合适"),
    )
    .await?;

if receipt.status == RefundStatus::Processing {
    // 异步落地：等回调，或稍后轮询（可挂到 phoenix-schedule）
    let settled = manager.sync_refund("wechat_native", "R-1001").await?;
}
```

### 退款异步通知（微信）

微信的退款回调**走独立的 URL**，不是支付回调那一个，且 URL 是**每笔退款请求**带上去的。因此：

- 配置项是单独的 `refund_notify_url`（不填就不带回调 URL，只能靠 `sync_refund` 轮询）；
- 路由也是单独的 `pay.notify.wechat.refund`；
- 投到退款路由的**支付**回调会被拒绝（`event_type` 必须是 `REFUND.*`）——两种 resource 结构不同，混用会把支付事件写进退款记录。

`PayManager::handle_refund_notify` 的保证：

- 先验签解密再取字段（与支付回调同一条硬规矩）；
- **金额要对得上**：回调金额与库里那笔退款不一致直接报错，不静默记录；
- 幂等：网关会重投，重复回调返回 `AlreadyProcessed`，不二次迁移；
- 只报「仍在处理中」的回调被确认但不算迁移；
- 落地后同步订单状态（成功累计够 → `Refunded`；全失败 → 回到 `Paid`）。

`REFUND.ABNORMAL` 映射为 `Failed`：它表示退款**没有**成功、需要人工在商户平台处理，把它挂在 pending 上等于永远等不到结果。

`PayManager::refund` 的顺序是刻意的——**先落库，再调网关**：

1. 校验；`out_refund_no` 已存在则直接幂等返回旧回执，**不再动钱**；
2. 检查订单处于 `Paid` / `Refunding`、`total` 与库里订单金额一致、剩余可退额够本次；
3. 以 `Processing` 落库；
4. 调用网关；
5. 记录结果，并同步订单自身状态。

由此得到的性质：

- **可退额 = 订单金额 − 所有「未失败」退款之和**。在途（`Processing`）的退款也占额，因此重复提交不会超退；
- 网关报错时，那笔退款被标记 `Failed`（额度释放，可换号重试），但**行仍在**——网关成功却在返回路上崩溃的场景有据可查，不会静默丢钱；
- 同一个 `out_refund_no` 用在**另一张订单**上直接拒绝（`InvalidRefund`）。

`PaymentProvider` 上的三个方法都有默认实现（`NotImplemented`），自研渠道可以只实现下单/通知/查询。

## 对账

```rust
// day_start = 该账单日在真实时间轴上的起点（网关按 UTC+8 出账）
let result = manager.reconcile_day("wechat_native", "2026-07-25", day_start).await?;
if !result.is_balanced() {
    for issue in &result.discrepancies { /* 报警 / 人工处理 */ }
}
```

比对是**双向**的，因为两个方向的错法不一样：

| 差异 | 含义 |
| --- | --- |
| `MissingLocally` | 账单结算了一笔我们**没有**记录的订单——货没发，或通知丢了 |
| `MissingRemotely` | 我们认为已付，账单里**没有**——白发货，或结算在另一天 |
| `AmountMismatch` | 双方都认得这笔单，但金额不一致 |
| `StatusMismatch` | 双方都认得这笔单，但状态不一致 |

要点：

- `reconcile(bill, local)` 是**纯函数**，不做 I/O：账单来自 `PaymentProvider::download_bill`，本地侧来自 `PaymentStore::paid_within(provider, from, to)`（半开区间 `[from, to)`）。想用别处拿到的账单就调 `PayManager::reconcile_bill`。
- **时区不在本 crate 里**：`day_start` 由调用方给出。两个国内网关都按 UTC+8 出账，本 crate 不带时区库，也不猜。
- 本地 `Refunding` / `Refunded` 与账单上的 `Paid` **算一致**：退款晚于付款结算，付款日的账单当然还记着这笔。
- 账单里非 `Paid` 的行，本地没有记录时不报差异（没有钱要对）。
- 表头与状态值同时认识规范英文、微信的 UTF-8 中文、支付宝的 **GBK** 中文，会剥掉微信的反引号前缀，遇到不认识的状态值**报错而不是当成已付**——那正是对账要抓的错。按「仅成功」下载的账单没有状态列时整份按 `Paid` 读。

## 微信 / 支付宝接入现状：真实网关已实现

实现位于 `crates/phoenix-pay/src/gateway.rs`（provider 装配）+ `wechat.rs` / `alipay.rs`（协议纯函数）+ `crypto.rs`（RSA-SHA256 签名/验签、AES-256-GCM 解密，`ring` / `aes-gcm`）+ `transport.rs`（`PayHttp` 传输接缝）。

**已实现：**

- **微信 APIv3（Native 扫码）**
  - 请求签名：`{METHOD}\n{path+query}\n{timestamp}\n{nonce}\n{body}\n` 商户私钥 RSA-SHA256（PKCS#1 v1.5），`Authorization: WECHATPAY2-SHA256-RSA2048 mchid=...,serial_no=...`；私钥从 `private_key_path` 懒加载（支持 PKCS#8 / PKCS#1 PEM）。
  - 应答 / 回调验签：`Wechatpay-Timestamp/Nonce/Signature/Serial` 头，签名串 `{timestamp}\n{nonce}\n{body}\n`，按 `serial` 选择平台证书验签，时间戳超出 ±300s 拒绝（重放窗口）。
  - 平台证书：`GET /v3/certificates` 下载，`encrypt_certificate` 用 APIv3 密钥 AES-256-GCM 解密；进程内缓存（按 serial，TTL 12h），未知 serial 强制重拉一次；`/v3/certificates` 应答本身用「刚解出的证书」自举验签，验不过整批丢弃。也可用 `platform_cert_path` 预置证书文件（离线验签；文件证书不轮换，换证书需替换文件并重启——自动轮换是后续项）。
  - 回调 `resource` AES-256-GCM 解密（nonce + associated_data 取自回调体），`trade_state` 映射状态机（`SUCCESS→Paid`、`NOTPAY/USERPAYING/ACCEPT→Pending`、`CLOSED/REVOKED→Closed`、`PAYERROR→Failed`、`REFUND→Refunding`）。
  - 接口：`POST /v3/pay/transactions/native` 下单（→ `code_url` → `PaymentAction::QrCode`）、`GET /v3/pay/transactions/out-trade-no/{no}?mchid=` 查询（404 → `OrderNotFound`）、`POST .../out-trade-no/{no}/close` 关单（204 也验签）。
- **支付宝当面付（RSA2）**
  - 请求签名：公共参数 + `biz_content`，除 `sign` 外按 key 字典序 `k=v&k=v`（值不 urlencode，空值跳过，`sign_type` 参与签名）SHA256withRSA，base64；`timestamp` 为 GMT+8 `yyyy-MM-dd HH:mm:ss`（纯整数历法换算，无浮点无 chrono）。
  - 同步应答验签：取 `alipay_trade_xxx_response` 的**原文子串**（字符串感知的括号配对，防止值里的 `{}` 干扰）用支付宝公钥验签，验签通过后才解析字段；`code != 10000` 报 `PayError::Gateway`，`ACQ.TRADE_NOT_EXIST` → `OrderNotFound`。
  - 异步通知验签：form 表单解码后，除 `sign`/`sign_type` 外字典序拼串 RSA2 验签；验签通过后再校验 `app_id` 与本渠道一致（防跨应用重放），`trade_status` 映射（`TRADE_SUCCESS/TRADE_FINISHED→Paid`、`WAIT_BUYER_PAY→Pending`、`TRADE_CLOSED→Closed`）。
  - 接口：`alipay.trade.precreate`（→ `qr_code`）/ `alipay.trade.query` / `alipay.trade.close`；网关地址取 `gateway_url`（沙箱可覆盖）。
- **金额换算集中一处**：`Amount::decimal_string()`（分→元字符串，`10001 → "100.01"`）与 `Amount::cny_from_decimal_str()`（元字符串→分），全整数运算，单测覆盖 1 分 / 100 分 / 10001 分。
- 密钥格式宽容：`Secret` 里的私钥 / 公钥同时接受 PEM（PKCS#8 / PKCS#1 / SPKI）与支付宝控制台风格的裸 base64。

**退款与对账（已实现）：**

- **微信**：`POST /v3/refund/domestic/refunds` 退款、`GET /v3/refund/domestic/refunds/{out_refund_no}` 查询；状态映射 `SUCCESS→Succeeded`、`PROCESSING→Processing`、`CLOSED`/`ABNORMAL→Failed`（`ABNORMAL` 表示需人工处理、钱**没有**退出，所以按失败计）。
- **微信对账单**：`GET /v3/bill/tradebill?bill_date=&bill_type=SUCCESS` 拿签名下载票据，再签名 GET 取 CSV；下载文件本身没有验签头，因此**用票据里公布的 SHA1 摘要校验后才解析**，摘要不符直接报错。
- **支付宝**：`alipay.trade.refund` 退款（总是带 `out_request_no` 幂等键）、`alipay.trade.fastpay.refund.query` 查询；该接口同步落地，没有 `Processing` 态。查询返回「成功但空体」表示这笔退款不存在，映射为 `RefundNotFound` 而**不是**零元退款。
- **支付宝对账单**：`download_bill` 已打通全流程——查签名 URL → 下载 ZIP → 解压 → 解析。ZIP 里有明细与汇总两个成员，**不靠文件名猜**（文件名同样是 GBK）：把每个成员都喂给解析器，取行数最多的那个，不是交易明细的成员根本匹配不上表头。`bill_url(date)` 仍然保留，用于归档原始文件；它是短期凭据，**不要打日志**。
- **GBK 不转码**：支付宝账单是 GBK，转码需要一张本 crate 不该携带的编码表。做法是**在字节层匹配**表头与状态值（每个别名同时带 UTF-8 与 GBK 两种拼写），而真正读取的列（订单号、交易号、十进制金额）在两种编码下都是 ASCII。`parse_bill_csv_bytes` 是驱动走的入口，`parse_bill_csv` 是它的 UTF-8 便捷包装。
- **ZIP 读取器是最小实现**：中央目录 + 本地头 + stored/DEFLATE，不支持加密 / ZIP64 / 分卷。所有长度都对缓冲区做边界检查，解压总量**先封顶再解**（256 MiB）——声称能膨胀到 1 TB 的账单是被拒绝，而不是被尝试。DEFLATE 用 `flate2`，它本来就在 lock 里（MySQL / Redis 驱动带的），没有引入新的第三方 crate。

**仍未实现（后续清单）：**
- 微信平台证书**自动轮换细节**：`platform_cert_path` 文件模式不自动重载；下载模式仅按 TTL / 未知 serial 重拉，未实现新旧证书重叠期的平滑切换策略。
- 微信 JSAPI / H5 / App / 小程序支付，支付宝 WAP / PC 网页支付、公钥**证书模式**（`app_cert_path` / `alipay_root_cert_path` 字段已预留，逻辑未接）。
- 币种仅 CNY（`Currency` 增员会在网关处强制编译期处理）。

### 配置示例

```toml
# config/pay.toml（示例；密钥值经 .env 注入，不要提交仓库）
[pay.wechat_native]
app_id = "wx1234567890"
mch_id = "1900000001"
mch_serial_no = "5157F09EFDC096DE15EBE81A47057A72"   # 商户 API 证书序列号
api_v3_key = "${WECHAT_API_V3_KEY}"                   # 32 字节 APIv3 密钥
private_key_path = "storage/certs/apiclient_key.pem"  # 商户私钥（PKCS#8）
# platform_cert_path = "storage/certs/wechatpay_platform.pem"  # 可选：预置平台证书（离线验签）
notify_url = "https://shop.example.com/pay/notify/wechat"

[pay.alipay_f2f]
app_id = "2021000000000000"
# gateway_url = "https://openapi-sandbox.dl.alipaydev.com/gateway.do"  # 沙箱联调时覆盖
app_private_key = "${ALIPAY_APP_PRIVATE_KEY}"    # 应用私钥（PEM 或裸 base64）
alipay_public_key = "${ALIPAY_PUBLIC_KEY}"       # 支付宝公钥（非应用公钥！）
notify_url = "https://shop.example.com/pay/notify/alipay"
```

```rust
use std::sync::Arc;
use phoenix_pay::prelude::*;

let wechat = WechatNativeProvider::new(wechat_config);   // 生产网关 + HyperPayHttp
let alipay = AlipayF2FProvider::new(alipay_config);      // 网关地址取 config.gateway_url
let manager = Arc::new(
    PayManager::builder()
        .provider(Arc::new(wechat))
        .provider(Arc::new(alipay))
        .build(),
);
// 测试 / 沙箱：WechatNativeProvider::with_transport(config, Arc::new(HyperPayHttp::new()), "http://127.0.0.1:PORT")
```

### 安全提示（务必遵守）

- **回调必须验签**：两个 provider 的 `verify_notify` 都是先验签（微信还要解密）后取数，任何字段在验签前都是攻击者可控输入；不要绕过 `PayManager::handle_notify` 自行解析回调体。
- **金额以服务端订单为准**：回调里的金额字段仅作审计参考（存于 `NotifyEvent::raw` / `payments.notify_payload`），发货判定依据是本地 `payments` 行的 `amount` + 状态机迁移，防止「一分钱买断」类金额篡改。
- 回调路由**不要**挂 Session/CSRF 中间件（见上文），真实性由验签保证；验签失败一律 400，不要为了「兼容」放行。
- 微信通知时间戳超出 ±300s 拒绝，重放同一通知因幂等只会得到 `AlreadyProcessed`。
- 私钥 / APIv3 密钥放 `.env` 或证书文件，用 `Secret` 承载（Debug/日志自动脱敏）；`tests/fixtures/` 下的密钥是 openssl 现场生成的测试专用材料，与任何真实商户无关。
- 沙箱联调建议：支付宝用 `gateway_url` 指向沙箱网关；微信无公开沙箱，先用 `with_transport` 对着本仓库的假网关集成测试（`crates/phoenix-pay/tests/gateway.rs`）回归，再以小额真实交易灰度验证。

## 合规提醒

- 个人开发者**无法直连**微信支付 / 支付宝的商户收单产品：直连需要企业主体（营业执照）申请商户号 / 签约当面付。
- 个人或小团队做聚合收款，通常经**持牌第三方聚合服务商 / 收单机构**间连；接入前核对其支付业务许可，避免「二清」资金池风险。
- 密钥（APIv3 key、RSA 私钥）放 `.env` / 证书文件，不进仓库；`Secret` 类型已保证日志与 `Debug` 不外泄。

## 验收

```bash
cargo test -p phoenix-pay
cargo clippy -p phoenix-pay --all-targets -- -D warnings
```
