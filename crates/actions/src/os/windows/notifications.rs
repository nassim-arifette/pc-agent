//! Per-user AUMID + COM activation for unpackaged Win32, with a bounded local worker.
use crate::notifications::{valid_id, xml, Callback, Delivery, Event, Operation, Request, APP_ID};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, ffi::c_void, sync::mpsc, time::Duration};
use windows::{
    core::{implement, IUnknown, Interface, Ref, Result, BOOL, GUID, HSTRING, PCWSTR},
    Data::Xml::Dom::XmlDocument,
    Foundation::TypedEventHandler,
    Win32::{
        Foundation::{CLASS_E_NOAGGREGATION, E_INVALIDARG, E_POINTER},
        System::Com::{
            CoInitializeEx, CoRegisterClassObject, CoRevokeClassObject, CoUninitialize,
            IClassFactory, IClassFactory_Impl, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
            REGCLS_MULTIPLEUSE,
        },
        UI::Notifications::{
            INotificationActivationCallback, INotificationActivationCallback_Impl,
            NOTIFICATION_USER_INPUT_DATA,
        },
    },
    UI::Notifications::{
        NotificationSetting, ToastFailedEventArgs, ToastNotification, ToastNotificationManager,
    },
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};

const CLSID: GUID = GUID::from_u128(0xc108c021_0295_4be1_b29d_1af1c3487047);
const CLSID_TEXT: &str = "{C108C021-0295-4BE1-B29D-1AF1C3487047}";

#[implement(INotificationActivationCallback)]
struct Activator(Callback);

impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(
        &self,
        app: &PCWSTR,
        arguments: &PCWSTR,
        _: *const NOTIFICATION_USER_INPUT_DATA,
        _: u32,
    ) -> Result<()> {
        // SAFETY: COM marshals both null-terminated input strings for the duration of Activate.
        let (app, arguments) = unsafe { (activation_text(*app)?, activation_text(*arguments)?) };
        if app != APP_ID || !valid_id(&arguments) {
            return Err(E_INVALIDARG.into());
        }
        (self.0)(Event::Activated(arguments));
        Ok(())
    }
}

unsafe fn activation_text(pointer: PCWSTR) -> Result<String> {
    if pointer.is_null() {
        return Err(E_POINTER.into());
    }
    let mut units = Vec::with_capacity(64);
    for index in 0..=64 {
        // SAFETY: caller supplies a valid null-terminated COM input string. Stop at its first
        // terminator; never allocate an unbounded amount for an external activation argument.
        let unit = unsafe { *pointer.0.add(index) };
        if unit == 0 {
            return String::from_utf16(&units).map_err(|_| E_INVALIDARG.into());
        }
        units.push(unit);
    }
    Err(E_INVALIDARG.into())
}

#[implement(IClassFactory)]
struct Factory(Callback);

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if iid.is_null() || object.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM supplies a writable out pointer; initialize it on every failure path.
        unsafe { *object = std::ptr::null_mut() };
        if !outer.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let callback: INotificationActivationCallback = Activator(self.0.clone()).into();
        // SAFETY: the COM caller supplies a valid IID and out pointer; QueryInterface owns the
        // returned reference independently of the temporary callback.
        unsafe { callback.query(iid, object).ok() }
    }
    fn LockServer(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
}

struct Registration(u32);
impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: this cookie and apartment were created on this worker and drop on it.
        unsafe {
            let _ = CoRevokeClassObject(self.0);
            CoUninitialize();
        }
    }
}

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the registry handle returned by RegCreateKeyExW.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn set_value(key: &str, name: &str, value: &str) -> Result<()> {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, name, value) = (wide(key), wide(name), wide(value));
    let mut handle = std::ptr::null_mut();
    // SAFETY: all strings are null terminated, the out handle is writable, no elevated hive.
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        )
    };
    if code != 0 {
        return Err(windows::core::Error::from_hresult(
            windows::core::HRESULT::from_win32(code),
        ));
    }
    let handle = Key(handle);
    // SAFETY: UTF-16 buffer includes the terminator and remains alive for its exact byte length.
    let code = unsafe {
        RegSetValueExW(
            handle.0,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    if code != 0 {
        return Err(windows::core::Error::from_hresult(
            windows::core::HRESULT::from_win32(code),
        ));
    }
    Ok(())
}

fn register(callback: Callback) -> Result<Registration> {
    let exe = std::env::current_exe().map_err(|_| E_INVALIDARG)?;
    let exe = exe.to_str().ok_or(E_INVALIDARG)?;
    if exe.contains('"') || exe.contains('\0') {
        return Err(E_INVALIDARG.into());
    }
    let key = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
    set_value(&key, "DisplayName", "PC Agent")?;
    set_value(&key, "CustomActivator", CLSID_TEXT)?;
    set_value(
        &format!(r"Software\Classes\CLSID\{CLSID_TEXT}\LocalServer32"),
        "",
        &format!("\"{exe}\" --toast-activated"),
    )?;
    // SAFETY: dedicated fresh thread; its apartment is paired with Registration::drop.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
    let factory: IClassFactory = Factory(callback).into();
    // SAFETY: factory is a valid COM object; COM retains its reference until revocation.
    match unsafe {
        CoRegisterClassObject(&CLSID, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)
    } {
        Ok(cookie) => Ok(Registration(cookie)),
        Err(error) => {
            // SAFETY: balance the successful initialization above.
            unsafe { CoUninitialize() };
            Err(error)
        }
    }
}

fn key(id: &str) -> HSTRING {
    // The 16-character group/tag works on older Windows 10, too. No paths are in the shell.
    HSTRING::from(
        Sha256::digest(id.as_bytes())[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
    )
}

pub(crate) fn run(receiver: mpsc::Receiver<Request>, callback: Callback) {
    let Ok(_registration) = register(callback.clone()) else {
        return;
    };
    let app = HSTRING::from(APP_ID);
    let Ok(notifier) = ToastNotificationManager::CreateToastNotifierWithId(&app) else {
        return;
    };
    let mut retained = VecDeque::<(String, ToastNotification)>::new();
    for request in receiver {
        if request.at.elapsed() >= Duration::from_secs(5) {
            continue;
        }
        let result = (|| -> Result<Delivery> {
            match request.operation {
                Operation::Status => Ok(if notifier.Setting()? == NotificationSetting::Enabled {
                    Delivery::Enabled
                } else {
                    Delivery::Disabled
                }),
                Operation::Show(n) => {
                    if notifier.Setting()? != NotificationSetting::Enabled {
                        return Ok(Delivery::Disabled);
                    }
                    let document = XmlDocument::new()?;
                    document.LoadXml(&HSTRING::from(xml(&n)))?;
                    let toast = ToastNotification::CreateToastNotification(&document)?;
                    toast.SetTag(&key(&n.alert_id))?;
                    toast.SetGroup(&key(&n.watch_id))?;
                    let id = n.alert_id;
                    let failed = callback.clone();
                    toast.Failed(
                        &TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(
                            move |_, _| {
                                failed(Event::Failed(id.clone()));
                                Ok(())
                            },
                        ),
                    )?;
                    notifier.Show(&toast)?;
                    retained.push_back((n.watch_id, toast));
                    while retained.len() > 32 {
                        retained.pop_front();
                    }
                    Ok(Delivery::Sent)
                }
                Operation::Remove(id) => {
                    ToastNotificationManager::History()?.RemoveGroupWithId(&key(&id), &app)?;
                    retained.retain(|(watch, _)| watch != &id);
                    Ok(Delivery::Sent)
                }
                Operation::Clear => {
                    ToastNotificationManager::History()?.ClearWithId(&app)?;
                    retained.clear();
                    Ok(Delivery::Sent)
                }
            }
        })()
        .unwrap_or(Delivery::Failed);
        let _ = request.reply.try_send(result);
    }
}
