//! Daily reconciliation (对账): compare a provider's bill against local orders.
//!
//! The provider's bill is the authority on what actually settled. Reconciling
//! answers the only question that matters at close of business: **does every
//! yuan the provider says it moved match a local order in the same state, and
//! vice versa?** Both directions are checked, because they fail differently:
//!
//! - the provider has a paid line we never recorded → we shipped nothing, or a
//!   notification was lost;
//! - we have a paid order the bill does not list → we shipped for free, or the
//!   order settled on a different day;
//! - the amounts or statuses differ → someone is wrong about the money.
//!
//! [`reconcile`] is pure: it takes a [`Bill`] and the local records for the
//! same window and returns a [`Reconciliation`]. Fetching either side is the
//! caller's job ([`PaymentProvider::download_bill`](crate::PaymentProvider::download_bill)
//! and [`PaymentStore::paid_within`](crate::PaymentStore::paid_within)), which
//! keeps the comparison itself trivially testable and free of I/O.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Amount, PayError, PaymentRecord, PaymentStatus};

/// One settled line of a provider bill.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BillEntry {
    /// Merchant order number.
    pub out_trade_no: String,
    /// Provider-side transaction id, when the bill carries one.
    pub transaction_id: Option<String>,
    /// Amount the provider settled.
    pub amount: Amount,
    /// Amount already refunded against this order, as of the bill.
    pub refunded: Amount,
    /// Normalized status of the line.
    pub status: PaymentStatus,
}

/// A provider bill for one calendar day.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Bill {
    /// Provider key that produced the bill.
    pub provider: String,
    /// Bill date in `YYYY-MM-DD`, in the provider's own timezone.
    pub date: String,
    /// Settled lines.
    pub entries: Vec<BillEntry>,
}

impl Bill {
    /// Sum of every settled amount, minus what was refunded.
    ///
    /// # Errors
    ///
    /// Returns [`PayError::AmountOverflow`] or [`PayError::CurrencyMismatch`]
    /// when the lines cannot be summed.
    pub fn net_total(&self) -> Result<Amount, PayError> {
        let mut total = Amount::from_minor(0, self.currency());
        for entry in &self.entries {
            if entry.status != PaymentStatus::Paid {
                continue;
            }
            total = total.checked_add(entry.amount)?;
            let net = total
                .minor()
                .checked_sub(entry.refunded.minor())
                .ok_or(PayError::AmountOverflow)?;
            total = Amount::from_minor(net, total.currency());
        }
        Ok(total)
    }

    fn currency(&self) -> crate::Currency {
        self.entries
            .first()
            .map_or_else(crate::Currency::default, |entry| entry.amount.currency())
    }
}

/// One way a bill line and a local order disagree.
///
/// Every variant names the order it is about, so an operator can act on it
/// without re-deriving anything.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Discrepancy {
    /// The bill settled an order we have no local record of at all.
    MissingLocally {
        /// Merchant order number from the bill.
        out_trade_no: String,
        /// Amount the provider settled.
        amount: Amount,
    },
    /// We consider the order paid, but the bill does not list it.
    MissingRemotely {
        /// Merchant order number of the local record.
        out_trade_no: String,
        /// Amount we recorded.
        amount: Amount,
    },
    /// Both sides know the order but disagree on how much moved.
    AmountMismatch {
        /// Merchant order number.
        out_trade_no: String,
        /// What we recorded.
        local: Amount,
        /// What the bill says.
        remote: Amount,
    },
    /// Both sides know the order but disagree on its state.
    StatusMismatch {
        /// Merchant order number.
        out_trade_no: String,
        /// What we recorded.
        local: PaymentStatus,
        /// What the bill says.
        remote: PaymentStatus,
    },
}

impl Discrepancy {
    /// The order number this discrepancy is about.
    #[must_use]
    pub fn out_trade_no(&self) -> &str {
        match self {
            Self::MissingLocally { out_trade_no, .. }
            | Self::MissingRemotely { out_trade_no, .. }
            | Self::AmountMismatch { out_trade_no, .. }
            | Self::StatusMismatch { out_trade_no, .. } => out_trade_no,
        }
    }
}

/// Result of comparing one bill against the local records for the same window.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Reconciliation {
    /// Provider key.
    pub provider: String,
    /// Bill date (`YYYY-MM-DD`).
    pub date: String,
    /// Orders that agreed on both amount and status.
    pub matched: usize,
    /// Everything that did not agree, sorted by order number.
    pub discrepancies: Vec<Discrepancy>,
}

impl Reconciliation {
    /// Whether the two sides agreed on every order.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        self.discrepancies.is_empty()
    }
}

/// Compare a bill against the local records covering the same window.
///
/// `local` should be what
/// [`PaymentStore::paid_within`](crate::PaymentStore::paid_within) returned for
/// the bill's day. Records for a different provider are ignored rather than
/// reported, so passing a wider query is harmless.
///
/// Only bill lines the provider marks [`PaymentStatus::Paid`] are expected to
/// have a local paid order; closed and failed lines are matched when we know
/// them and skipped when we do not, since there is no money to account for.
#[must_use]
pub fn reconcile(bill: &Bill, local: &[PaymentRecord]) -> Reconciliation {
    let mut local_by_number: BTreeMap<&str, &PaymentRecord> = BTreeMap::new();
    for record in local {
        if record.provider == bill.provider {
            local_by_number.insert(record.out_trade_no.as_str(), record);
        }
    }

    let mut matched = 0;
    let mut discrepancies = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();

    for entry in &bill.entries {
        seen.insert(entry.out_trade_no.as_str());
        let Some(record) = local_by_number.get(entry.out_trade_no.as_str()) else {
            if entry.status == PaymentStatus::Paid {
                discrepancies.push(Discrepancy::MissingLocally {
                    out_trade_no: entry.out_trade_no.clone(),
                    amount: entry.amount,
                });
            }
            continue;
        };
        let mut agreed = true;
        if record.amount != entry.amount {
            agreed = false;
            discrepancies.push(Discrepancy::AmountMismatch {
                out_trade_no: entry.out_trade_no.clone(),
                local: record.amount,
                remote: entry.amount,
            });
        }
        if !status_agrees(record.status, entry.status) {
            agreed = false;
            discrepancies.push(Discrepancy::StatusMismatch {
                out_trade_no: entry.out_trade_no.clone(),
                local: record.status,
                remote: entry.status,
            });
        }
        if agreed {
            matched += 1;
        }
    }

    for (out_trade_no, record) in &local_by_number {
        if seen.contains(*out_trade_no) {
            continue;
        }
        discrepancies.push(Discrepancy::MissingRemotely {
            out_trade_no: (*out_trade_no).to_owned(),
            amount: record.amount,
        });
    }

    discrepancies.sort_by(|left, right| {
        left.out_trade_no()
            .cmp(right.out_trade_no())
            .then_with(|| format!("{left:?}").cmp(&format!("{right:?}")))
    });

    Reconciliation {
        provider: bill.provider.clone(),
        date: bill.date.clone(),
        matched,
        discrepancies,
    }
}

/// Whether a local status and a bill status describe the same reality.
///
/// A refund is settled asynchronously, so a locally `Refunding` / `Refunded`
/// order still shows as paid on the trade bill for the day it was paid. Those
/// pairs agree; everything else must match exactly.
fn status_agrees(local: PaymentStatus, remote: PaymentStatus) -> bool {
    match (local, remote) {
        (left, right) if left == right => true,
        (
            PaymentStatus::Refunding | PaymentStatus::Refunded,
            PaymentStatus::Paid | PaymentStatus::Refunded,
        ) => true,
        _ => false,
    }
}

/// One accepted spelling of a column name or status value.
///
/// Both encodings are carried because the two gateways disagree: `WeChat`
/// publishes UTF-8, Alipay publishes **GBK**, and GBK cannot be transcoded
/// without an encoding table this crate has no business shipping. Matching on
/// raw bytes sidesteps that entirely — and the values we actually read
/// (order numbers, amounts) are ASCII in both encodings.
struct Alias {
    /// UTF-8 spelling, also used by the canonical English form.
    utf8: &'static str,
    /// GBK spelling, when the name is Chinese.
    gbk: &'static [u8],
}

impl Alias {
    /// Whether a raw CSV cell is this name in either encoding.
    fn matches(&self, cell: &[u8]) -> bool {
        cell == self.utf8.as_bytes() || (!self.gbk.is_empty() && cell == self.gbk)
    }
}

/// ASCII-only alias (the canonical English column names).
const fn ascii(utf8: &'static str) -> Alias {
    Alias { utf8, gbk: b"" }
}

/// Column names accepted for each normalized column.
const HEADER_ALIASES: &[(&str, &[Alias])] = &[
    (
        "out_trade_no",
        &[
            ascii("out_trade_no"),
            Alias {
                utf8: "商户订单号",
                gbk: b"\xC9\xCC\xBB\xA7\xB6\xA9\xB5\xA5\xBA\xC5",
            },
            Alias {
                utf8: "商家订单号",
                gbk: b"\xC9\xCC\xBC\xD2\xB6\xA9\xB5\xA5\xBA\xC5",
            },
            Alias {
                utf8: "商户单号",
                gbk: b"\xC9\xCC\xBB\xA7\xB5\xA5\xBA\xC5",
            },
        ],
    ),
    (
        "transaction_id",
        &[
            ascii("transaction_id"),
            Alias {
                utf8: "微信订单号",
                gbk: b"\xCE\xA2\xD0\xC5\xB6\xA9\xB5\xA5\xBA\xC5",
            },
            Alias {
                utf8: "支付宝交易号",
                gbk: b"\xD6\xA7\xB8\xB6\xB1\xA6\xBD\xBB\xD2\xD7\xBA\xC5",
            },
            Alias {
                utf8: "交易号",
                gbk: b"\xBD\xBB\xD2\xD7\xBA\xC5",
            },
        ],
    ),
    (
        "amount",
        &[
            ascii("amount"),
            Alias {
                utf8: "订单金额",
                gbk: b"\xB6\xA9\xB5\xA5\xBD\xF0\xB6\xEE",
            },
            Alias {
                utf8: "订单金额（元）",
                gbk: b"\xB6\xA9\xB5\xA5\xBD\xF0\xB6\xEE\xA3\xA8\xD4\xAA\xA3\xA9",
            },
            Alias {
                utf8: "总金额",
                gbk: b"\xD7\xDC\xBD\xF0\xB6\xEE",
            },
            Alias {
                utf8: "应结订单金额",
                gbk: b"\xD3\xA6\xBD\xE1\xB6\xA9\xB5\xA5\xBD\xF0\xB6\xEE",
            },
        ],
    ),
    (
        "refunded",
        &[
            ascii("refunded"),
            Alias {
                utf8: "退款金额",
                gbk: b"\xCD\xCB\xBF\xEE\xBD\xF0\xB6\xEE",
            },
            Alias {
                utf8: "商户退款金额",
                gbk: b"\xC9\xCC\xBB\xA7\xCD\xCB\xBF\xEE\xBD\xF0\xB6\xEE",
            },
            Alias {
                utf8: "退款金额（元）",
                gbk: b"\xCD\xCB\xBF\xEE\xBD\xF0\xB6\xEE\xA3\xA8\xD4\xAA\xA3\xA9",
            },
        ],
    ),
    (
        "status",
        &[
            ascii("status"),
            Alias {
                utf8: "交易状态",
                gbk: b"\xBD\xBB\xD2\xD7\xD7\xB4\xCC\xAC",
            },
            Alias {
                utf8: "业务类型",
                gbk: b"\xD2\xB5\xCE\xF1\xC0\xE0\xD0\xCD",
            },
        ],
    ),
];

/// Status values accepted in the `status` column, mapped onto the state
/// machine. An unrecognized value is an error, never a guess.
const BILL_STATUSES: &[(&[Alias], PaymentStatus)] = &[
    (
        &[
            ascii("SUCCESS"),
            ascii("TRADE_SUCCESS"),
            ascii("TRADE_FINISHED"),
            Alias {
                utf8: "支付",
                gbk: b"\xD6\xA7\xB8\xB6",
            },
            Alias {
                utf8: "交易",
                gbk: b"\xBD\xBB\xD2\xD7",
            },
        ],
        PaymentStatus::Paid,
    ),
    (
        &[
            ascii("REFUND"),
            Alias {
                utf8: "退款",
                gbk: b"\xCD\xCB\xBF\xEE",
            },
        ],
        PaymentStatus::Refunded,
    ),
    (
        &[
            ascii("CLOSED"),
            ascii("REVOKED"),
            ascii("TRADE_CLOSED"),
            Alias {
                utf8: "关闭",
                gbk: b"\xB9\xD8\xB1\xD5",
            },
        ],
        PaymentStatus::Closed,
    ),
    (&[ascii("PAYERROR")], PaymentStatus::Failed),
];

/// Parse a provider bill from CSV text.
///
/// Handles the daily bill both CN gateways publish plus the canonical English
/// form: the header row is matched through [`HEADER_ALIASES`], amounts are
/// decimal yuan strings, and `WeChat`'s backtick cell prefix is stripped.
///
/// The `status` column is optional. Bills are normally downloaded with the
/// provider's "successful trades only" filter, where every row is a settled
/// payment, so a bill without a status column is read as all-[`PaymentStatus::Paid`]
/// rather than rejected.
///
/// # Errors
///
/// Returns [`PayError::Reconcile`] for a missing header, a header with no
/// recognizable order-number or amount column, a malformed row, or an amount
/// that is not a valid decimal.
pub fn parse_bill_csv(provider: &str, date: &str, csv: &str) -> Result<Bill, PayError> {
    parse_bill_csv_bytes(provider, date, csv.as_bytes())
}

/// Parse a provider bill from raw CSV bytes.
///
/// Alipay publishes its bill as GBK, which is not valid UTF-8, so the bytes
/// entry point is the one the drivers use: header cells and status values are
/// matched against both encodings (see [`Alias`]), and the values actually
/// read — order numbers, transaction ids, decimal amounts — are ASCII in
/// either. Any other column may be non-UTF-8 and is never decoded.
///
/// # Errors
///
/// Same as [`parse_bill_csv`].
pub fn parse_bill_csv_bytes(provider: &str, date: &str, csv: &[u8]) -> Result<Bill, PayError> {
    let mut lines = csv
        .split(|byte| *byte == b'\n')
        .map(trim_ascii)
        // Both gateways wrap the detail rows in `#`-prefixed comment and
        // summary blocks.
        .filter(|line| !line.is_empty() && !line.starts_with(b"#"));
    let header = lines
        .next()
        .ok_or_else(|| PayError::Reconcile("bill has no header row".to_owned()))?;
    let columns: Vec<&[u8]> = split_cells(header);
    let index = |name: &str| {
        let aliases = HEADER_ALIASES
            .iter()
            .find(|(canonical, _)| *canonical == name)
            .map_or(&[] as &[Alias], |(_, aliases)| *aliases);
        columns
            .iter()
            .position(|column| aliases.iter().any(|alias| alias.matches(column)))
            .ok_or_else(|| PayError::Reconcile(format!("bill has no `{name}` column")))
    };
    let out_trade_no = index("out_trade_no")?;
    let transaction_id = index("transaction_id").ok();
    let amount = index("amount")?;
    let refunded = index("refunded").ok();
    let status = index("status").ok();

    let mut entries = Vec::new();
    for (row, line) in lines.enumerate() {
        let cells = split_cells(line);
        let cell = |position: usize| {
            cells.get(position).copied().ok_or_else(|| {
                PayError::Reconcile(format!("bill row {} is missing a column", row + 1))
            })
        };
        // Bills end with a summary block whose rows do not carry an order
        // number; stop there rather than reporting them as malformed.
        let number = cell(out_trade_no)?;
        if number.is_empty() {
            break;
        }
        entries.push(BillEntry {
            out_trade_no: text(number),
            transaction_id: transaction_id
                .and_then(|position| cells.get(position).copied())
                .filter(|value| !value.is_empty())
                .map(text),
            amount: Amount::cny_from_decimal_str(&text(cell(amount)?))?,
            refunded: match refunded.and_then(|position| cells.get(position).copied()) {
                Some(value) if !value.is_empty() => Amount::cny_from_decimal_str(&text(value))?,
                _ => Amount::cny(0),
            },
            status: match status {
                Some(position) => parse_bill_status(cell(position)?)?,
                None => PaymentStatus::Paid,
            },
        });
    }

    Ok(Bill {
        provider: provider.to_owned(),
        date: date.to_owned(),
        entries,
    })
}

/// Map a bill's status cell onto the order state machine.
///
/// Accepts the canonical lowercase names plus the vocabularies both gateways
/// print, in either encoding. Unknown values are an error rather than a guess:
/// silently treating an unrecognized state as paid is exactly the mistake
/// reconciliation exists to catch.
fn parse_bill_status(cell: &[u8]) -> Result<PaymentStatus, PayError> {
    for (aliases, status) in BILL_STATUSES {
        if aliases.iter().any(|alias| alias.matches(cell)) {
            return Ok(*status);
        }
    }
    let decoded = text(cell);
    decoded
        .parse::<PaymentStatus>()
        .map_err(|_| PayError::Reconcile(format!("unknown bill status `{decoded}`")))
}

/// Split one CSV line into cleaned cells.
fn split_cells(line: &[u8]) -> Vec<&[u8]> {
    line.split(|byte| *byte == b',').map(clean_cell).collect()
}

/// Decode a cell leniently. Only ever applied to columns whose values are
/// ASCII in both encodings, so the lossy path is unreachable in practice and
/// harmless if a gateway ever surprises us.
fn text(cell: &[u8]) -> String {
    String::from_utf8_lossy(cell).into_owned()
}

fn trim_ascii(line: &[u8]) -> &[u8] {
    let mut line = line;
    while let [first, rest @ ..] = line {
        if first.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = line {
        if last.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    line
}

/// Strip the quoting the gateways use (`WeChat` prefixes every cell with a
/// backtick so spreadsheets keep long numbers as text).
fn clean_cell(cell: &[u8]) -> &[u8] {
    let mut cell = trim_ascii(cell);
    while let [b'`' | b'"', rest @ ..] = cell {
        cell = rest;
    }
    while let [rest @ .., b'"'] = cell {
        cell = rest;
    }
    trim_ascii(cell)
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::CreateOrder;

    fn record(out_trade_no: &str, minor: u64, status: PaymentStatus) -> PaymentRecord {
        let mut record = PaymentRecord::new(
            "mock",
            &CreateOrder::new(out_trade_no, Amount::cny(minor), "subject"),
            SystemTime::UNIX_EPOCH,
        );
        record.status = status;
        record
    }

    fn entry(out_trade_no: &str, minor: u64, status: PaymentStatus) -> BillEntry {
        BillEntry {
            out_trade_no: out_trade_no.to_owned(),
            transaction_id: Some(format!("TX-{out_trade_no}")),
            amount: Amount::cny(minor),
            refunded: Amount::cny(0),
            status,
        }
    }

    fn bill(entries: Vec<BillEntry>) -> Bill {
        Bill {
            provider: "mock".to_owned(),
            date: "2026-07-25".to_owned(),
            entries,
        }
    }

    #[test]
    fn a_matching_day_is_balanced() {
        let result = reconcile(
            &bill(vec![
                entry("T-1", 990, PaymentStatus::Paid),
                entry("T-2", 5, PaymentStatus::Paid),
            ]),
            &[
                record("T-1", 990, PaymentStatus::Paid),
                record("T-2", 5, PaymentStatus::Paid),
            ],
        );
        assert!(result.is_balanced());
        assert_eq!(result.matched, 2);
        assert_eq!(result.date, "2026-07-25");
    }

    #[test]
    fn reports_both_directions_of_missing_orders() {
        let result = reconcile(
            &bill(vec![entry("only-remote", 100, PaymentStatus::Paid)]),
            &[record("only-local", 200, PaymentStatus::Paid)],
        );
        assert_eq!(result.matched, 0);
        // Sorted by order number, so `only-local` comes first.
        assert_eq!(
            result.discrepancies,
            vec![
                Discrepancy::MissingRemotely {
                    out_trade_no: "only-local".to_owned(),
                    amount: Amount::cny(200),
                },
                Discrepancy::MissingLocally {
                    out_trade_no: "only-remote".to_owned(),
                    amount: Amount::cny(100),
                },
            ]
        );
    }

    #[test]
    fn reports_amount_and_status_mismatches_separately() {
        let result = reconcile(
            &bill(vec![
                entry("T-1", 990, PaymentStatus::Paid),
                entry("T-2", 700, PaymentStatus::Paid),
            ]),
            &[
                record("T-1", 890, PaymentStatus::Paid),
                record("T-2", 700, PaymentStatus::Pending),
            ],
        );
        assert_eq!(result.matched, 0);
        assert_eq!(
            result.discrepancies,
            vec![
                Discrepancy::AmountMismatch {
                    out_trade_no: "T-1".to_owned(),
                    local: Amount::cny(890),
                    remote: Amount::cny(990),
                },
                Discrepancy::StatusMismatch {
                    out_trade_no: "T-2".to_owned(),
                    local: PaymentStatus::Pending,
                    remote: PaymentStatus::Paid,
                },
            ]
        );
    }

    #[test]
    fn a_refunding_order_still_agrees_with_a_paid_bill_line() {
        let result = reconcile(
            &bill(vec![
                entry("T-1", 990, PaymentStatus::Paid),
                entry("T-2", 990, PaymentStatus::Paid),
            ]),
            &[
                record("T-1", 990, PaymentStatus::Refunding),
                record("T-2", 990, PaymentStatus::Refunded),
            ],
        );
        assert!(result.is_balanced(), "{:?}", result.discrepancies);
        assert_eq!(result.matched, 2);
    }

    #[test]
    fn unpaid_bill_lines_we_never_saw_are_not_discrepancies() {
        let result = reconcile(&bill(vec![entry("T-9", 100, PaymentStatus::Closed)]), &[]);
        assert!(result.is_balanced());
        assert_eq!(result.matched, 0, "nothing was compared");
    }

    #[test]
    fn records_from_other_providers_are_ignored() {
        let mut foreign = record("T-1", 990, PaymentStatus::Paid);
        foreign.provider = "other".to_owned();
        let result = reconcile(
            &bill(vec![entry("T-1", 990, PaymentStatus::Paid)]),
            &[foreign],
        );
        assert_eq!(
            result.discrepancies,
            vec![Discrepancy::MissingLocally {
                out_trade_no: "T-1".to_owned(),
                amount: Amount::cny(990),
            }]
        );
    }

    #[test]
    fn parses_bill_csv_with_quoting_and_a_summary_block() {
        let csv = "\
out_trade_no,transaction_id,amount,refunded,status
`T-1,`4200001,9.90,0.00,paid
`T-2,`4200002,0.05,0.05,paid
,,,,
总交易单数,交易总金额
2,9.95
";
        let bill = parse_bill_csv("wechat_native", "2026-07-25", csv).expect("parse");
        assert_eq!(bill.provider, "wechat_native");
        assert_eq!(bill.entries.len(), 2);
        assert_eq!(bill.entries[0].out_trade_no, "T-1");
        assert_eq!(bill.entries[0].transaction_id.as_deref(), Some("4200001"));
        assert_eq!(bill.entries[0].amount, Amount::cny(990));
        assert_eq!(bill.entries[1].refunded, Amount::cny(5));
        assert_eq!(bill.net_total().expect("net"), Amount::cny(990));
    }

    #[test]
    fn parses_a_wechat_style_bill_with_chinese_headers() {
        let csv = "\
交易时间,微信订单号,商户订单号,交易状态,订单金额,退款金额
`2026-07-25 10:00:00,`4200001,`T-1,SUCCESS,9.90,0.00
`2026-07-25 11:00:00,`4200002,`T-2,REFUND,1.00,1.00
";
        let bill = parse_bill_csv("wechat_native", "2026-07-25", csv).expect("parse");
        assert_eq!(bill.entries.len(), 2);
        assert_eq!(bill.entries[0].status, PaymentStatus::Paid);
        assert_eq!(bill.entries[0].amount, Amount::cny(990));
        assert_eq!(bill.entries[0].transaction_id.as_deref(), Some("4200001"));
        assert_eq!(bill.entries[1].status, PaymentStatus::Refunded);
        assert_eq!(bill.entries[1].refunded, Amount::cny(100));
    }

    #[test]
    fn a_success_only_bill_without_a_status_column_is_all_paid() {
        let csv = "商户订单号,订单金额\nT-1,9.90\nT-2,0.05\n";
        let bill = parse_bill_csv("alipay_f2f", "2026-07-25", csv).expect("parse");
        assert_eq!(bill.entries.len(), 2);
        assert!(
            bill.entries
                .iter()
                .all(|entry| entry.status == PaymentStatus::Paid)
        );
        assert_eq!(bill.net_total().expect("net"), Amount::cny(995));
    }

    #[test]
    fn parses_a_gbk_bill_without_transcoding_it() {
        // Alipay publishes GBK. The header and status cells are matched on raw
        // bytes; the values we read are ASCII in either encoding.
        let mut csv: Vec<u8> = Vec::new();
        csv.extend_from_slice("#支付宝交易明细查询\n".as_bytes());
        // 商户订单号,业务类型,订单金额（元）
        csv.extend_from_slice(
            b"\xC9\xCC\xBB\xA7\xB6\xA9\xB5\xA5\xBA\xC5,\
              \xD2\xB5\xCE\xF1\xC0\xE0\xD0\xCD,\
              \xB6\xA9\xB5\xA5\xBD\xF0\xB6\xEE\xA3\xA8\xD4\xAA\xA3\xA9\n",
        );
        csv.extend_from_slice(b"T-1,\xBD\xBB\xD2\xD7,9.90\n"); // 交易
        csv.extend_from_slice(b"T-2,\xCD\xCB\xBF\xEE,0.05\n"); // 退款

        assert!(
            std::str::from_utf8(&csv).is_err(),
            "the fixture is not UTF-8"
        );
        let bill = parse_bill_csv_bytes("alipay_f2f", "2026-07-25", &csv).expect("parse");
        assert_eq!(bill.entries.len(), 2);
        assert_eq!(bill.entries[0].out_trade_no, "T-1");
        assert_eq!(bill.entries[0].amount, Amount::cny(990));
        assert_eq!(bill.entries[0].status, PaymentStatus::Paid);
        assert_eq!(bill.entries[1].status, PaymentStatus::Refunded);
    }

    #[test]
    fn rejects_bills_it_cannot_read() {
        assert!(parse_bill_csv("mock", "2026-07-25", "").is_err());
        assert!(
            parse_bill_csv("mock", "2026-07-25", "a,b,c\n1,2,3").is_err(),
            "an unknown column layout must fail, not silently yield zero entries"
        );
        assert!(
            parse_bill_csv(
                "mock",
                "2026-07-25",
                "out_trade_no,amount,status\nT-1,not-a-number,paid"
            )
            .is_err()
        );
        assert!(
            parse_bill_csv(
                "mock",
                "2026-07-25",
                "out_trade_no,amount,status\nT-1,1.00,teleported"
            )
            .is_err(),
            "an unknown status must fail rather than default to paid"
        );
    }
}
