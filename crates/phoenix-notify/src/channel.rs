/// Delivery channel requested by a [`Notification`](crate::Notification).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ChannelKind {
    /// Outbound email via a configured [`phoenix_mail::Mailer`].
    Mail,
    /// The `notifications` table via a [`NotificationStore`](crate::NotificationStore).
    Database,
}

impl ChannelKind {
    /// Stable lowercase channel name (`mail` / `database`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mail => "mail",
            Self::Database => "database",
        }
    }
}

impl std::fmt::Display for ChannelKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_channel_names() {
        assert_eq!(ChannelKind::Mail.as_str(), "mail");
        assert_eq!(ChannelKind::Database.as_str(), "database");
        assert_eq!(ChannelKind::Database.to_string(), "database");
    }
}
