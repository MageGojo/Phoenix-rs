# phoenix-pay 测试密钥（仅测试用途）

本目录所有密钥 / 证书均由 `openssl` 现场生成、**只用于单元与集成测试**，
从未在任何真实商户 / 支付平台注册，不承载任何机密。请勿在生产环境使用。

| 文件 | 生成方式 | 用途 |
| --- | --- | --- |
| `wechat_merchant_key.pem` | `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048` (PKCS#8) | 模拟微信商户 API 私钥（请求签名） |
| `wechat_merchant_pub.pem` | `openssl pkey -pubout` | 假网关校验客户端 `Authorization` 签名 |
| `wechat_platform_key.pem` | 同上 (PKCS#8) | 假网关模拟微信平台私钥（应答/回调签名） |
| `wechat_platform_cert.pem` | `openssl req -x509 -set_serial 0x5157...6733` | 模拟微信平台证书（客户端验签、序列号选择） |
| `alipay_app_key.pem` | `openssl genrsa -traditional` (PKCS#1) | 模拟支付宝应用私钥（覆盖 PKCS#1 解析分支） |
| `alipay_app_pub.pem` | `openssl rsa -pubout` | 假网关校验请求 `sign` |
| `alipay_platform_key.pem` | `openssl genpkey` (PKCS#8) | 假网关模拟支付宝平台私钥 |
| `alipay_platform_pub.pem` | `openssl pkey -pubout` | 客户端验签同步应答与异步通知 |
