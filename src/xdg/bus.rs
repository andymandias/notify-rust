use crate::xdg::{NOTIFICATION_DEFAULT_BUS, NOTIFICATION_PORTAL_BUS};

fn skip_first_slash(s: &str) -> &str {
    if let Some('/') = s.chars().next() {
        &s[1..]
    } else {
        s
    }
}

use std::{fmt, path::PathBuf};

type BusNameType = std::borrow::Cow<'static, str>;

#[derive(Clone, Debug)]
pub struct NotificationBus(BusNameType);

impl Default for NotificationBus {
    #[cfg(feature = "zbus")]
    fn default() -> Self {
        Self(
            zbus::names::WellKnownName::from_static_str(NOTIFICATION_DEFAULT_BUS)
                .unwrap()
                .to_string()
                .into(),
        )
    }

    #[cfg(all(feature = "dbus", not(feature = "zbus")))]
    fn default() -> Self {
        Self(
            dbus::strings::BusName::from_slice(NOTIFICATION_DEFAULT_BUS)
                .unwrap()
                .to_string()
                .into(),
        )
    }
}

#[cfg(all(feature = "dbus", not(feature = "zbus")))]
impl fmt::Display for NotificationBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl NotificationBus {
    fn namespaced_custom(custom_path: &str) -> Option<String> {
        // abusing path for semantic join
        skip_first_slash(
            PathBuf::from("/de/hoodie/Notification")
                .join(custom_path)
                .to_str()?,
        )
        .replace('/', ".")
        .into()
    }

    #[cfg(feature = "zbus")]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[cfg(feature = "zbus")]
    pub fn custom(custom_path: &str) -> Option<Self> {
        let name =
            zbus::names::WellKnownName::try_from(Self::namespaced_custom(custom_path)?).ok()?;
        Some(Self(name.to_string().into()))
    }

    #[cfg(all(feature = "dbus", not(feature = "zbus")))]
    pub fn custom(custom_path: &str) -> Option<Self> {
        let name = dbus::strings::BusName::new(Self::namespaced_custom(custom_path)?).ok()?;
        Some(Self(name.to_string().into()))
    }

    pub fn into_name(self) -> BusNameType {
        self.0
    }

    #[cfg(feature = "zbus")]
    pub fn portal() -> Self {
        Self(
            zbus::names::WellKnownName::from_static_str(NOTIFICATION_PORTAL_BUS)
                .unwrap()
                .to_string()
                .into(),
        )
    }

    #[cfg(all(feature = "dbus", not(feature = "zbus")))]
    pub fn portal() -> Self {
        Self(
            dbus::strings::BusName::from_slice(NOTIFICATION_PORTAL_BUS)
                .unwrap()
                .to_string()
                .into(),
        )
    }
}
