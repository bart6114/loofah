mod activation;

use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use ::windows::{
    Data::Xml::Dom::XmlDocument,
    Foundation::{IPropertyValue, TypedEventHandler},
    UI::Notifications::{
        NotificationSetting, ToastActivatedEventArgs, ToastDismissalReason, ToastNotification,
        ToastNotificationManager,
    },
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    },
    core::{HSTRING, Interface, Result},
};

use crate::Notification;

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub(crate) enum Action {
    Confirm,
    Accept,
    Dismiss,
    Timeout,
    Option,
    Footer,
}
type Handler = Arc<dyn Fn(String, i32) + Send + Sync>;
static HANDLERS: LazyLock<Mutex<HashMap<Action, Handler>>> = LazyLock::new(Mutex::default);
static TOASTS: LazyLock<Mutex<HashMap<String, (String, ToastNotification, usize)>>> =
    LazyLock::new(Mutex::default);
static APP_ID: OnceLock<String> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Apartment(bool);
impl Apartment {
    fn new() -> Result<Self> {
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self(true)),
            Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self(false)),
            Err(error) => Err(error),
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { RoUninitialize() };
        }
    }
}

pub fn set_app_id(id: String) {
    let _ = APP_ID.set(id.clone());
    if let Err(error) = activation::register(&id) {
        tracing::error!(%error, "windows_notification_registration_failed");
    }
}

pub(crate) fn set_handler(action: Action, handler: Handler) {
    HANDLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(action, handler);
}

fn dispatch(key: &str, action: Action, option: i32) {
    let toast = TOASTS.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
    let Some((key, _toast, _)) = toast else {
        return;
    };
    let handler = HANDLERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&action)
        .cloned();
    if let Some(handler) = handler {
        handler(key, option);
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn xml(notification: &Notification, token: &str) -> String {
    let mut xml = format!(
        "<toast launch=\"{token}:confirm\" duration=\"long\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text>",
        escape(&notification.title),
        escape(&notification.message)
    );
    if let Some(footer) = &notification.footer {
        xml.push_str(&format!(
            "<text placement=\"attribution\">{}</text>",
            escape(&footer.text)
        ));
    }
    xml.push_str("</binding></visual><actions>");
    if let Some(options) = notification
        .options
        .as_ref()
        .filter(|items| !items.is_empty())
    {
        xml.push_str("<input id=\"choice\" type=\"selection\" title=\"Choose an option\" defaultInput=\"0\">");
        for (index, option) in options.iter().enumerate() {
            xml.push_str(&format!(
                "<selection id=\"{index}\" content=\"{}\"/>",
                escape(option)
            ));
        }
        xml.push_str("</input>");
        xml.push_str(&format!(
            "<action content=\"{}\" arguments=\"{token}:option\" activationType=\"foreground\"/>",
            escape(
                notification
                    .action_label
                    .as_deref()
                    .unwrap_or("Start recording")
            )
        ));
    } else if let Some(label) = &notification.action_label {
        xml.push_str(&format!(
            "<action content=\"{}\" arguments=\"{token}:accept\" activationType=\"foreground\"/>",
            escape(label)
        ));
    }
    if let Some(footer) = &notification.footer {
        xml.push_str(&format!(
            "<action content=\"{}\" arguments=\"{token}:footer\" activationType=\"foreground\"/>",
            escape(&footer.action_label)
        ));
    }
    xml.push_str(&format!("<action content=\"Dismiss\" arguments=\"{token}:dismiss\" activationType=\"foreground\"/></actions></toast>"));
    xml
}

pub(crate) fn show(notification: &Notification) {
    if let Err(error) = show_inner(notification) {
        tracing::error!(%error, "windows_notification_failed");
    }
}

fn show_inner(notification: &Notification) -> Result<()> {
    let _apartment = Apartment::new()?;
    let id = APP_ID.get().ok_or_else(|| {
        ::windows::core::Error::new(
            ::windows::Win32::Foundation::E_UNEXPECTED,
            "Notification app ID missing",
        )
    })?;
    let key = notification
        .key
        .clone()
        .unwrap_or_else(|| format!("toast-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    crate::store_context(&key, notification.source.clone());
    let token = uuid::Uuid::new_v4().to_string();
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(xml(notification, &token)))?;
    let toast = ToastNotification::CreateToastNotification(&document)?;
    let activation_key = token.clone();
    let option_count = notification.options.as_ref().map_or(0, Vec::len);
    toast.Activated(&TypedEventHandler::new(
        move |_, args: ::windows::core::Ref<::windows::core::IInspectable>| {
            let args = args
                .as_ref()
                .ok_or_else(::windows::core::Error::empty)?
                .cast::<ToastActivatedEventArgs>()?;
            let argument = args.Arguments()?.to_string();
            let (action, option) = match argument.rsplit(':').next().unwrap_or_default() {
                "accept" => (Action::Accept, -1),
                "footer" => (Action::Footer, -1),
                "dismiss" => (Action::Dismiss, -1),
                "option" => {
                    let value = args
                        .UserInput()?
                        .Lookup(&HSTRING::from("choice"))?
                        .cast::<IPropertyValue>()?
                        .GetString()?
                        .to_string();
                    let Some(index) = value.parse::<usize>().ok().filter(|i| *i < option_count)
                    else {
                        return Ok(());
                    };
                    (Action::Option, index as i32)
                }
                _ => (Action::Confirm, -1),
            };
            dispatch(&activation_key, action, option);
            Ok(())
        },
    ))?;
    let dismissal_key = token.clone();
    toast.Dismissed(
        &TypedEventHandler::new(
            move |_,
                  args: ::windows::core::Ref<
                ::windows::UI::Notifications::ToastDismissedEventArgs,
            >| {
                if let Some(args) = args.as_ref() {
                    match args.Reason()? {
                        ToastDismissalReason::UserCanceled => {
                            dispatch(&dismissal_key, Action::Dismiss, -1)
                        }
                        // Windows controls popup duration; the application timeout is tracked separately.
                        ToastDismissalReason::TimedOut => {}
                        _ => {}
                    }
                }
                Ok(())
            },
        ),
    )?;
    let failure_key = token.clone();
    toast.Failed(&TypedEventHandler::new(
        move |_, args: ::windows::core::Ref<::windows::UI::Notifications::ToastFailedEventArgs>| {
            let code = args.as_ref().and_then(|args| args.ErrorCode().ok());
            tracing::error!(?code, "windows_notification_delivery_failed");
            TOASTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&failure_key);
            Ok(())
        },
    ))?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(id))?;
    let setting = notifier.Setting()?;
    if setting != NotificationSetting::Enabled {
        tracing::warn!(?setting, "windows_notifications_disabled");
        return Ok(());
    }
    TOASTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), (key.clone(), toast.clone(), option_count));
    if let Err(error) = notifier.Show(&toast) {
        TOASTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&token);
        return Err(error);
    }
    let timeout = notification.timeout;
    std::thread::spawn(move || {
        std::thread::sleep(timeout.unwrap_or(crate::CONTEXT_TTL));
        if timeout.is_some() {
            dispatch(&token, Action::Timeout, -1);
        } else {
            TOASTS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&token);
        }
        if let Ok(_apartment) = Apartment::new() {
            if let Some(id) = APP_ID.get() {
                if let Ok(notifier) =
                    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(id))
                {
                    let _ = notifier.Hide(&toast);
                }
            }
        }
    });
    Ok(())
}

pub(crate) fn clear() {
    let result = (|| -> Result<()> {
        let _apartment = Apartment::new()?;
        if let Some(id) = APP_ID.get() {
            let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(id))?;
            let toasts = std::mem::take(&mut *TOASTS.lock().unwrap_or_else(|e| e.into_inner()));
            for (_, (_, toast, _)) in toasts {
                notifier.Hide(&toast)?;
            }
            ToastNotificationManager::History()?.ClearWithId(&HSTRING::from(id))?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(%error, "windows_notification_clear_failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notification_content_cannot_inject_toast_actions() {
        let notification = Notification::builder()
            .title("<action/>")
            .message("A & B")
            .action_label("\"Start\"")
            .build();
        let xml = xml(&notification, "test");
        assert!(xml.contains("&lt;action/&gt;"));
        assert!(xml.contains("A &amp; B"));
        assert!(xml.contains("&quot;Start&quot;"));
    }
}

pub fn shutdown() {
    clear();
    activation::shutdown();
}

pub use activation::uninstall;
