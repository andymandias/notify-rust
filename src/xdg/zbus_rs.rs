use crate::{error::*, hints::Hint, notification::Notification, urgency::Urgency, xdg};
use zbus::{export::futures_util::TryStreamExt, zvariant, MatchRule};
use std::{collections::HashMap, fs, sync::atomic::{AtomicU32, Ordering}};

use super::{bus::NotificationBus, ActionResponse, ActionResponseHandler, CloseReason};

pub mod bus {

    use crate::xdg::{NOTIFICATION_DEFAULT_BUS, NOTIFICATION_PORTAL_BUS};

    fn skip_first_slash(s: &str) -> &str {
        if let Some('/') = s.chars().next() {
            &s[1..]
        } else {
            s
        }
    }

    use std::path::PathBuf;

    type BusNameType = zbus::names::WellKnownName<'static>;

    #[derive(Clone, Debug)]
    pub struct NotificationBus(BusNameType);

    impl Default for NotificationBus {
        #[cfg(feature = "zbus")]
        fn default() -> Self {
            Self(zbus::names::WellKnownName::from_static_str(NOTIFICATION_DEFAULT_BUS).unwrap())
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

        pub fn as_str(&self) -> &str {
            self.0.as_str()
        }

        pub fn custom(custom_path: &str) -> Option<Self> {
            let name =
                zbus::names::WellKnownName::try_from(Self::namespaced_custom(custom_path)?).ok()?;
            Some(Self(name))
        }

        pub fn into_name(self) -> BusNameType {
            self.0
        }

        pub fn portal() -> Self {
            Self(zbus::names::WellKnownName::from_static_str(NOTIFICATION_PORTAL_BUS).unwrap())
        }
    }
}

/// A handle to a shown notification.
///
/// This keeps a connection alive to ensure actions work on certain desktops.
#[derive(Debug)]
pub struct ZbusNotificationHandle {
    pub(crate) id: u32,
    pub(crate) connection: zbus::Connection,
    pub(crate) notification: Notification,
}

impl ZbusNotificationHandle {
    pub(crate) fn new(
        id: u32,
        connection: zbus::Connection,
        notification: Notification,
    ) -> ZbusNotificationHandle {
        ZbusNotificationHandle {
            id,
            connection,
            notification,
        }
    }

    pub async fn wait_for_action(self, invocation_closure: impl ActionResponseHandler) {
        wait_for_action_signal(&self.connection, self.id, invocation_closure).await;
    }

    pub async fn close_fallible(self) -> Result<()> {
        self.connection
            .call_method(
                Some(self.notification.bus.clone().into_name()),
                xdg::NOTIFICATION_OBJECTPATH,
                Some(xdg::NOTIFICATION_INTERFACE),
                "CloseNotification",
                &(self.id),
            )
            .await?;
        Ok(())
    }

    pub async fn close(self) {
        self.close_fallible().await.unwrap();
    }

    pub fn on_close<F>(self, closure: F)
    where
        F: FnOnce(CloseReason),
    {
        zbus::block_on(self.wait_for_action(|action: &ActionResponse| {
            if let ActionResponse::Closed(reason) = action {
                closure(*reason);
            }
        }));
    }

    pub fn update_fallible(&mut self) -> Result<()> {
        self.id = zbus::block_on(send_notification_via_connection(
            &self.notification,
            self.id,
            &self.connection,
        ))?;
        Ok(())
    }

    pub fn update(&mut self) {
        self.update_fallible().unwrap();
    }
}

async fn send_notification_via_connection(
    notification: &Notification,
    id: u32,
    connection: &zbus::Connection,
) -> Result<u32> {
    let bus = if get_portal_version_via_connection(connection).await.is_ok_and(|version| version > 0) {
        NotificationBus::portal()
    } else {
        NotificationBus::default()
    };

    send_notification_via_connection_at_bus(
        notification,
        id,
        connection,
        bus,
    ).await
}

async fn send_notification_via_connection_at_bus(
    notification: &Notification,
    id: u32,
    connection: &zbus::Connection,
    bus: NotificationBus,
) -> Result<u32> {
    if bus.as_str() == xdg::NOTIFICATION_PORTAL_BUS {
        static APP_ID: AtomicU32 = AtomicU32::new(1);

        let id = if id == 0 {
            APP_ID.fetch_add(1, Ordering::Relaxed)
        } else {
            id
        };

        let mut dict = HashMap::<&str, &zvariant::Value>::new();

        let title_variant = zvariant::Value::from(&notification.summary);
        dict.insert("title", &title_variant);

        let body_variant = zvariant::Value::from(&notification.body);
        dict.insert("body", &body_variant);

        let priority_variant = zvariant::Value::from(
            if let Some(Hint::Urgency(urgency)) = notification
                .get_hints()
                .find(|hint| matches!(hint, Hint::Urgency(_)))
            {
                match urgency {
                    Urgency::Low => "low",
                    Urgency::Normal => "normal",
                    Urgency::Critical => "urgent",
                }
            } else {
                "normal"
            },
        );
        dict.insert("priority", &priority_variant);

        let mut icon_variant = zvariant::Value::from(("themed", zvariant::Value::from(vec![&notification.icon])));
        if notification.icon.is_empty() {
            if let Some(Hint::ImagePath(image_path)) = notification.get_hints().find(|hint| matches!(hint, Hint::ImagePath(_))) {
                if let Ok(image_data) = fs::read(image_path) {
                    icon_variant = zvariant::Value::from(("bytes", zvariant::Value::from(image_data)));
                }
            }
        }
        dict.insert("icon", &icon_variant);

        let _ = connection
            .call_method(
                Some(bus.into_name()),
                xdg::NOTIFICATION_PORTAL_OBJECTPATH,
                Some(xdg::NOTIFICATION_PORTAL_INTERFACE),
                "AddNotification",
                &(
                    id.to_string(),
                    dict,
                ),
            )
            .await?;
        Ok(id)
    } else {
        let reply: u32 = connection
            .call_method(
                Some(bus.into_name()),
                xdg::NOTIFICATION_OBJECTPATH,
                Some(xdg::NOTIFICATION_INTERFACE),
                "Notify",
                &(
                    &notification.appname,
                    id,
                    &notification.icon,
                    &notification.summary,
                    &notification.body,
                    &notification.actions,
                    crate::hints::hints_to_map(notification),
                    i32::from(notification.timeout),
                ),
            )
            .await?
            .body()
            .deserialize()?;
        Ok(reply)
    }
}

pub async fn connect_and_send_notification(
    notification: &Notification,
) -> Result<ZbusNotificationHandle> {
    let connection = zbus::Connection::session().await?;
    let inner_id = notification.id.unwrap_or(0);
    let bus = if get_portal_version_via_connection(&connection).await.is_ok_and(|version| version > 0) {
        NotificationBus::portal()
    } else {
        NotificationBus::default()
    };
    let id = send_notification_via_connection_at_bus(notification, inner_id, &connection, bus.clone()).await?;

    Ok(ZbusNotificationHandle::new(
        id,
        connection,
        Notification {
            bus,
            ..notification.clone()
        },
    ))
}

pub(crate) async fn connect_and_send_notification_at_bus(
    notification: &Notification,
    bus: NotificationBus,
) -> Result<ZbusNotificationHandle> {
    let connection = zbus::Connection::session().await?;
    let inner_id = notification.id.unwrap_or(0);
    let id = send_notification_via_connection_at_bus(notification, inner_id, &connection, bus).await?;

    Ok(ZbusNotificationHandle::new(
        id,
        connection,
        notification.clone(),
    ))
}

pub async fn get_capabilities_at_bus(bus: NotificationBus) -> Result<Vec<String>> {
    let connection = zbus::Connection::session().await?;
    let info: Vec<String> = connection
        .call_method(
            Some(bus.into_name()),
            xdg::NOTIFICATION_OBJECTPATH,
            Some(xdg::NOTIFICATION_INTERFACE),
            "GetCapabilities",
            &(),
        )
        .await?
        .body()
        .deserialize()?;
    Ok(info)
}

pub async fn get_capabilities() -> Result<Vec<String>> {
    get_capabilities_at_bus(Default::default()).await
}

pub async fn get_portal_version_via_connection(connection: &zbus::Connection) -> Result<u32> {
    let proxy = zbus::Proxy::new(
        connection,
        xdg::NOTIFICATION_PORTAL_BUS,
        xdg::NOTIFICATION_PORTAL_OBJECTPATH,
        xdg::NOTIFICATION_PORTAL_INTERFACE,
    ).await?;
    let version = proxy.get_property::<u32>("version").await?;
    Ok(version)
}

pub async fn get_server_information_at_bus(bus: NotificationBus) -> Result<xdg::ServerInformation> {
    let connection = zbus::Connection::session().await?;
    let info: xdg::ServerInformation = connection
        .call_method(
            Some(bus.into_name()),
            xdg::NOTIFICATION_OBJECTPATH,
            Some(xdg::NOTIFICATION_INTERFACE),
            "GetServerInformation",
            &(),
        )
        .await?
        .body()
        .deserialize()?;

    Ok(info)
}

pub async fn get_server_information() -> Result<xdg::ServerInformation> {
    get_server_information_at_bus(Default::default()).await
}

/// Listens for the `ActionInvoked(UInt32, String)` Signal.
///
/// No need to use this, check out `Notification::show_and_wait_for_action(FnOnce(action:&str))`
pub async fn handle_action(id: u32, func: impl ActionResponseHandler) {
    let connection = zbus::Connection::session().await.unwrap();
    wait_for_action_signal(&connection, id, func).await;
}

async fn wait_for_action_signal(
    connection: &zbus::Connection,
    id: u32,
    handler: impl ActionResponseHandler,
) {
    let action_signal_rule = MatchRule::builder()
        .msg_type(zbus::MessageType::Signal)
        .interface(xdg::NOTIFICATION_INTERFACE)
        .unwrap()
        .member("ActionInvoked")
        .unwrap()
        .build();

    let proxy = zbus::fdo::DBusProxy::new(connection).await.unwrap();
    proxy.add_match_rule(action_signal_rule).await.unwrap();

    let close_signal_rule = MatchRule::builder()
        .msg_type(zbus::MessageType::Signal)
        .interface(xdg::NOTIFICATION_INTERFACE)
        .unwrap()
        .member("NotificationClosed")
        .unwrap()
        .build();
    proxy.add_match_rule(close_signal_rule).await.unwrap();

    while let Ok(Some(msg)) = zbus::MessageStream::from(connection).try_next().await {
        let header = msg.header();
        if let zbus::MessageType::Signal = header.message_type() {
            match header.member() {
                Some(name) if name == "ActionInvoked" => {
                    match msg.body().deserialize::<(u32, String)>() {
                        Ok((nid, action)) if nid == id => {
                            handler.call(&ActionResponse::Custom(&action));
                            break;
                        }
                        _ => {}
                    }
                }
                Some(name) if name == "NotificationClosed" => {
                    match msg.body().deserialize::<(u32, u32)>() {
                        Ok((nid, reason)) if nid == id => {
                            handler.call(&ActionResponse::Closed(reason.into()));
                            break;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}
