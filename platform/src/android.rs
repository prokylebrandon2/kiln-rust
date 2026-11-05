//! The real phone: JNI calls on the `NativeActivity`, the NDK's configuration, and the entry point.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use jni::errors::Result as JniResult;
use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use kiln_app::{Event, Host, Look};
use ndk::configuration::UiModeNight;
use slint::android::AndroidApp;
use slint::android::android_activity::{MainEvent, PollEvent};

/// The handler `kiln-app` installs; Slint's event listener is made before the app exists, so both
/// share this slot.
type Handler = Rc<RefCell<Option<Box<dyn Fn(Event)>>>>;

/// The entry point. `NativeActivity` calls it on its own thread once the activity exists; the app
/// ends when it returns.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    if let Err(e) = start(app) {
        eprintln!("Kiln could not start: {e}");
    }
}

fn start(app: AndroidApp) -> Result<(), String> {
    let host = AndroidHost::new(app.clone()).map_err(|e| e.to_string())?;

    // The engine's temporary files go in the app cache, where Settings > Cache can see them, not in
    // /data/local/tmp (the default, which an app may not write to).
    let tmp = host.cache.join("engine-tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    // SAFETY: `set_var` is unsound only while another thread reads or writes the environment. This
    // is the first thing the app does, before it (or Slint) starts a thread of its own, and the
    // only other threads are the Java runtime's, which do not touch Rust's environment.
    unsafe { std::env::set_var("TMPDIR", &tmp) };

    let handler = host.handler.clone();
    slint::android::init_with_event_listener(app, move |event| {
        let Some(event) = translate(event) else {
            return;
        };
        if let Some(handler) = handler.borrow().as_ref() {
            handler(event);
        }
    })
    .map_err(|e| e.to_string())?;

    kiln_app::run(Rc::new(host)).map_err(|e| e.to_string())
}

/// The few lifecycle events the app cares about.
fn translate(event: &PollEvent<'_>) -> Option<Event> {
    let PollEvent::Main(main) = event else {
        return None;
    };
    match main {
        MainEvent::Resume { .. } => Some(Event::Resumed),
        MainEvent::Pause => Some(Event::Paused),
        MainEvent::ConfigChanged { .. } => Some(Event::ConfigChanged),
        MainEvent::LowMemory => Some(Event::LowMemory),
        _ => None,
    }
}

pub struct AndroidHost {
    app: AndroidApp,
    vm: JavaVM,
    activity: GlobalRef,
    cache: PathBuf,
    files: PathBuf,
    version: String,
    handler: Handler,
}

impl AndroidHost {
    fn new(app: AndroidApp) -> JniResult<Self> {
        // SAFETY: android-activity hands out the process's `JavaVM*` here, valid for as long as the
        // process lives.
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast())? };
        let activity = {
            let mut env = vm.attach_current_thread()?;
            // SAFETY: `activity_as_ptr` is a JNI global reference to the `NativeActivity`, valid
            // until the activity is destroyed. `JObject` does not delete the reference it wraps,
            // and a global reference of our own is taken before this thread detaches.
            let raw = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
            env.new_global_ref(&raw)?
        };
        let mut host = Self {
            app,
            vm,
            activity,
            cache: PathBuf::new(),
            files: PathBuf::new(),
            version: String::new(),
            handler: Rc::default(),
        };
        host.cache = host.with_env(|env, activity| file_path(env, activity, "getCacheDir"))?;
        host.files = host.with_env(|env, activity| file_path(env, activity, "getFilesDir"))?;
        host.version = host.with_env(version_name)?;
        Ok(host)
    }

    /// Run `f` with a JNI environment for this thread. A Java exception left pending by a failed
    /// call is cleared, so that it cannot crash the app later.
    fn with_env<T>(
        &self,
        f: impl FnOnce(&mut JNIEnv<'_>, &GlobalRef) -> JniResult<T>,
    ) -> JniResult<T> {
        let mut env = self.vm.attach_current_thread()?;
        let out = f(&mut env, &self.activity);
        if out.is_err() && env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        out
    }
}

/// `activity.<method>().getAbsolutePath()` for the methods that return a `java.io.File`.
fn file_path(env: &mut JNIEnv<'_>, activity: &GlobalRef, method: &str) -> JniResult<PathBuf> {
    let dir = env
        .call_method(activity, method, "()Ljava/io/File;", &[])?
        .l()?;
    let path = env
        .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;
    Ok(PathBuf::from(java_string(env, path)?))
}

fn java_string(env: &mut JNIEnv<'_>, object: JObject<'_>) -> JniResult<String> {
    let string = JString::from(object);
    let text: String = env.get_string(&string)?.into();
    Ok(text)
}

fn package_name(env: &mut JNIEnv<'_>, activity: &GlobalRef) -> JniResult<String> {
    let name = env
        .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
        .l()?;
    java_string(env, name)
}

/// `getPackageManager().getPackageInfo(getPackageName(), 0).versionName`
fn version_name(env: &mut JNIEnv<'_>, activity: &GlobalRef) -> JniResult<String> {
    let manager = env
        .call_method(
            activity,
            "getPackageManager",
            "()Landroid/content/pm/PackageManager;",
            &[],
        )?
        .l()?;
    let name = env
        .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
        .l()?;
    let info = env
        .call_method(
            &manager,
            "getPackageInfo",
            "(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;",
            &[JValue::Object(&name), JValue::Int(0)],
        )?
        .l()?;
    let version = env
        .get_field(&info, "versionName", "Ljava/lang/String;")?
        .l()?;
    java_string(env, version)
}

/// `getResources().getConfiguration().fontScale`
fn font_scale(env: &mut JNIEnv<'_>, activity: &GlobalRef) -> JniResult<f32> {
    let resources = env
        .call_method(
            activity,
            "getResources",
            "()Landroid/content/res/Resources;",
            &[],
        )?
        .l()?;
    let configuration = env
        .call_method(
            &resources,
            "getConfiguration",
            "()Landroid/content/res/Configuration;",
            &[],
        )?
        .l()?;
    env.get_field(&configuration, "fontScale", "F")?.f()
}

/// `Build.VERSION.SDK_INT < 30 || Environment.isExternalStorageManager()`
fn has_all_files_access(env: &mut JNIEnv<'_>) -> JniResult<bool> {
    let sdk = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
        .i()?;
    if sdk < 30 {
        return Ok(true);
    }
    env.call_static_method(
        "android/os/Environment",
        "isExternalStorageManager",
        "()Z",
        &[],
    )?
    .z()
}

/// `startActivity(new Intent(action, Uri.parse("package:" + getPackageName())))`, or, with no
/// package, `startActivity(new Intent(action))`.
fn start_settings(
    env: &mut JNIEnv<'_>,
    activity: &GlobalRef,
    action: &str,
    for_package: bool,
) -> JniResult<()> {
    let action = env.new_string(action)?;
    let intent = if for_package {
        let package = package_name(env, activity)?;
        let uri_text = env.new_string(format!("package:{package}"))?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&uri_text)],
            )?
            .l()?;
        env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?
    } else {
        env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&action)],
        )?
    };
    env.call_method(
        activity,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intent)],
    )?;
    Ok(())
}

impl Host for AndroidHost {
    fn cache_dir(&self) -> PathBuf {
        self.cache.clone()
    }

    fn files_dir(&self) -> PathBuf {
        self.files.clone()
    }

    fn version_name(&self) -> String {
        self.version.clone()
    }

    fn look(&self) -> Look {
        let config = self.app.config();
        let density = config.density().map_or(1.0, |dpi| dpi as f32 / 160.0);
        let font_scale = self.with_env(font_scale).unwrap_or(1.0);
        Look {
            dark: config.ui_mode_night() == UiModeNight::Yes,
            density,
            font_scale,
        }
    }

    fn has_all_files_access(&self) -> bool {
        // If the question cannot be asked, show the permission view rather than a broken list.
        self.with_env(|env, _| has_all_files_access(env))
            .unwrap_or(false)
    }

    fn open_all_files_settings(&self) {
        let result = self.with_env(|env, activity| {
            start_settings(
                env,
                activity,
                "android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION",
                true,
            )
        });
        if result.is_err() {
            // A phone without the per-app page still has the general list of apps.
            let _ = self.with_env(|env, activity| {
                start_settings(
                    env,
                    activity,
                    "android.settings.MANAGE_ALL_FILES_ACCESS_PERMISSION",
                    false,
                )
            });
        }
    }

    fn finish(&self) {
        let _ = self.with_env(|env, activity| {
            env.call_method(activity, "finish", "()V", &[])?;
            Ok(())
        });
    }

    fn set_event_handler(&self, handler: Box<dyn Fn(Event)>) {
        *self.handler.borrow_mut() = Some(handler);
    }
}
