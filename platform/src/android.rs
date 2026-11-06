//! The real phone: JNI calls on the `NativeActivity`, the NDK's configuration, and the entry point.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use jni::errors::Result as JniResult;
use jni::objects::{GlobalRef, JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};
use kiln_app::{Event, Host, Look, OpenApp, OpenWith, Volume};
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
    storage: PathBuf,
    version: String,
    handler: Handler,
}

impl AndroidHost {
    fn new(app: AndroidApp) -> JniResult<Self> {
        // SAFETY: android-activity hands out the process's `JavaVM*` here, valid for as long as the
        // process lives.
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast())? };
        let activity = {
            let env = vm.attach_current_thread()?;
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
            storage: PathBuf::new(),
            version: String::new(),
            handler: Rc::default(),
        };
        host.cache = host.with_env(|env, activity| file_path(env, activity, "getCacheDir"))?;
        host.files = host.with_env(|env, activity| file_path(env, activity, "getFilesDir"))?;
        host.storage = host.with_env(|env, _| storage_root(env))?;
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
        // This thread never returns to Java, so the local references a call makes would pile up
        // for ever; a local frame frees them when the call is done.
        let out = env.with_local_frame(32, |env| f(env, &self.activity));
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

/// `Environment.getExternalStorageDirectory().getAbsolutePath()`
fn storage_root(env: &mut JNIEnv<'_>) -> JniResult<PathBuf> {
    let dir = env
        .call_static_method(
            "android/os/Environment",
            "getExternalStorageDirectory",
            "()Ljava/io/File;",
            &[],
        )?
        .l()?;
    let path = env
        .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;
    Ok(PathBuf::from(java_string(env, path)?))
}

/// `TimeZone.getDefault().getOffset(epochSeconds * 1000) / 1000`
fn utc_offset(env: &mut JNIEnv<'_>, epoch_seconds: i64) -> JniResult<i32> {
    let zone = env
        .call_static_method(
            "java/util/TimeZone",
            "getDefault",
            "()Ljava/util/TimeZone;",
            &[],
        )?
        .l()?;
    let millis = env
        .call_method(
            &zone,
            "getOffset",
            "(J)I",
            &[JValue::Long(epoch_seconds * 1000)],
        )?
        .i()?;
    Ok(millis / 1000)
}

/// `new StatFs(path)`: `(getAvailableBytes(), getTotalBytes())`.
fn space_of(env: &mut JNIEnv<'_>, path: &str) -> JniResult<(u64, u64)> {
    let text = env.new_string(path)?;
    let stat = env.new_object(
        "android/os/StatFs",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&text)],
    )?;
    let free = env
        .call_method(&stat, "getAvailableBytes", "()J", &[])?
        .j()?;
    let total = env.call_method(&stat, "getTotalBytes", "()J", &[])?.j()?;
    Ok((free.max(0) as u64, total.max(0) as u64))
}

/// The mounted removable volumes of `getSystemService("storage").getStorageVolumes()`.
fn removable_volumes(env: &mut JNIEnv<'_>, activity: &GlobalRef) -> JniResult<Vec<Volume>> {
    let service = env.new_string("storage")?;
    let manager = env
        .call_method(
            activity,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&service)],
        )?
        .l()?;
    let list = env
        .call_method(&manager, "getStorageVolumes", "()Ljava/util/List;", &[])?
        .l()?;
    let count = env.call_method(&list, "size", "()I", &[])?.i()?;
    let mut out = Vec::new();
    for i in 0..count {
        let found = env.with_local_frame(16, |env| volume_at(env, activity, &list, i))?;
        out.extend(found);
    }
    Ok(out)
}

/// One entry of that list, if it is a mounted USB drive or SD card (never the phone's own storage).
fn volume_at(
    env: &mut JNIEnv<'_>,
    activity: &GlobalRef,
    list: &JObject<'_>,
    index: i32,
) -> JniResult<Option<Volume>> {
    let volume = env
        .call_method(list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(index)])?
        .l()?;
    let dir = env
        .call_method(&volume, "getDirectory", "()Ljava/io/File;", &[])?
        .l()?;
    if dir.is_null() {
        return Ok(None);
    }
    let primary = env.call_method(&volume, "isPrimary", "()Z", &[])?.z()?;
    let removable = env.call_method(&volume, "isRemovable", "()Z", &[])?.z()?;
    if primary || !removable {
        return Ok(None);
    }
    let state = env
        .call_method(&volume, "getState", "()Ljava/lang/String;", &[])?
        .l()?;
    // `Environment.MEDIA_MOUNTED`
    if state.is_null() || java_string(env, state)? != "mounted" {
        return Ok(None);
    }
    let label = env
        .call_method(
            &volume,
            "getDescription",
            "(Landroid/content/Context;)Ljava/lang/String;",
            &[JValue::Object(activity.as_obj())],
        )?
        .l()?;
    let label = if label.is_null() {
        String::new()
    } else {
        java_string(env, label)?
    };
    let path = env
        .call_method(&dir, "getPath", "()Ljava/lang/String;", &[])?
        .l()?;
    let path = PathBuf::from(java_string(env, path)?);
    let label = if label.is_empty() {
        let name = path.file_name().unwrap_or_default();
        name.to_string_lossy().into_owned()
    } else {
        label
    };
    Ok(Some(Volume { path, label }))
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

/// `getPackageManager().getPackageInfo(getPackageName(), 0).getLongVersionCode()` (API 28; the app
/// needs 30).
fn version_code(env: &mut JNIEnv<'_>, activity: &GlobalRef) -> JniResult<i64> {
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
    env.call_method(&info, "getLongVersionCode", "()J", &[])?
        .j()
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

/// Put `text` on the clipboard: `ClipData.newPlainText("Properties", text)` handed to
/// `getSystemService("clipboard").setPrimaryClip`.
fn put_clipboard(env: &mut JNIEnv<'_>, activity: &GlobalRef, text: &str) -> JniResult<()> {
    let name = env.new_string("clipboard")?;
    let manager = env
        .call_method(
            activity,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    let label = env.new_string("Properties")?;
    let content = env.new_string(text)?;
    let clip = env
        .call_static_method(
            "android/content/ClipData",
            "newPlainText",
            "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
            &[JValue::Object(&label), JValue::Object(&content)],
        )?
        .l()?;
    env.call_method(
        &manager,
        "setPrimaryClip",
        "(Landroid/content/ClipData;)V",
        &[JValue::Object(&clip)],
    )?;
    Ok(())
}

/// Forget a Java exception a failed call left pending, so that the next JNI call is allowed.
fn clear_pending(env: &mut JNIEnv<'_>) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}

/// The `_id` of the `MediaStore.Files` row whose `_data` is `path`, or `None` when the media
/// scanner does not know the file:
/// `contentResolver.query(base, ["_id"], "_data=?", [path], null)`.
fn media_id(
    env: &mut JNIEnv<'_>,
    activity: &GlobalRef,
    base: &JObject<'_>,
    path: &str,
) -> JniResult<Option<i64>> {
    let resolver = env
        .call_method(
            activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;
    let columns = env.new_object_array(1, "java/lang/String", JObject::null())?;
    let column = env.new_string("_id")?;
    env.set_object_array_element(&columns, 0, &column)?;
    let args = env.new_object_array(1, "java/lang/String", JObject::null())?;
    let value = env.new_string(path)?;
    env.set_object_array_element(&args, 0, &value)?;
    let selection = env.new_string("_data=?")?;
    let cursor = env
        .call_method(
            &resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
            &[
                JValue::Object(base),
                JValue::Object(&columns),
                JValue::Object(&selection),
                JValue::Object(&args),
                JValue::Object(&JObject::null()),
            ],
        )?
        .l()?;
    if cursor.is_null() {
        return Ok(None);
    }
    let found = env
        .call_method(&cursor, "moveToFirst", "()Z", &[])
        .and_then(|v| v.z());
    let id = match found {
        Ok(true) => env
            .call_method(&cursor, "getLong", "(I)J", &[JValue::Int(0)])
            .and_then(|v| v.j())
            .map(Some),
        Ok(false) => Ok(None),
        Err(e) => Err(e),
    };
    // The cursor is closed whatever happened above.
    let _ = env.call_method(&cursor, "close", "()V", &[]);
    clear_pending(env);
    id
}

/// Stop Android from refusing to hand out `file://` URIs: an empty `StrictMode.VmPolicy`
/// (`FileUriExposedException` is only thrown when the policy asks for it).
fn relax_file_uri_policy(env: &mut JNIEnv<'_>) -> JniResult<()> {
    let builder = env.new_object("android/os/StrictMode$VmPolicy$Builder", "()V", &[])?;
    let policy = env
        .call_method(&builder, "build", "()Landroid/os/StrictMode$VmPolicy;", &[])?
        .l()?;
    env.call_static_method(
        "android/os/StrictMode",
        "setVmPolicy",
        "(Landroid/os/StrictMode$VmPolicy;)V",
        &[JValue::Object(&policy)],
    )?;
    Ok(())
}

/// A URI for the file that other apps can read: `content://media/external/file/<id>` from
/// `MediaStore.Files`, found by `_data`. When the media scanner has no row for it (or the query
/// fails) it is `file://` instead, after relaxing StrictMode; some receiving apps cannot read that.
/// The result is a local reference: use it inside the same `with_env` call.

fn file_uri<'a>(env: &mut JNIEnv<'a>, activity: &GlobalRef, path: &str) -> JniResult<JObject<'a>> {
    let volume = env.new_string("external")?;
    let base = env
        .call_static_method(
            "android/provider/MediaStore$Files",
            "getContentUri",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&volume)],
        )?
        .l()?;
    let id = media_id(env, activity, &base, path).unwrap_or_else(|_| {
        clear_pending(env);
        None
    });
    if let Some(id) = id {
        return env
            .call_static_method(
                "android/content/ContentUris",
                "withAppendedId",
                "(Landroid/net/Uri;J)Landroid/net/Uri;",
                &[JValue::Object(&base), JValue::Long(id)],
            )?
            .l();
    }
    relax_file_uri_policy(env)?;
    let name = env.new_string(path)?;
    let file = env.new_object(
        "java/io/File",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&name)],
    )?;
    env.call_static_method(
        "android/net/Uri",
        "fromFile",
        "(Ljava/io/File;)Landroid/net/Uri;",
        &[JValue::Object(&file)],
    )?
    .l()
}

/// A Java `CharSequence` (or `String`) as a Rust string, through `toString()`.
fn char_seq(env: &mut JNIEnv<'_>, object: &JObject<'_>) -> JniResult<String> {
    let text = env
        .call_method(object, "toString", "()Ljava/lang/String;", &[])?
        .l()?;
    let text = JString::from(text);
    Ok(env.get_string(&text)?.into())
}

/// `MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)`; `None` when Android has no type.
fn mime_of(env: &mut JNIEnv<'_>, ext: &str) -> JniResult<Option<String>> {
    let map = env
        .call_static_method(
            "android/webkit/MimeTypeMap",
            "getSingleton",
            "()Landroid/webkit/MimeTypeMap;",
            &[],
        )?
        .l()?;
    let ext = env.new_string(ext)?;
    let mime = env
        .call_method(
            &map,
            "getMimeTypeFromExtension",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&ext)],
        )?
        .l()?;
    if mime.is_null() {
        return Ok(None);
    }
    char_seq(env, &mime).map(Some)
}

/// `viewIntent`: `ACTION_VIEW` (or `ACTION_EDIT`) for the file's URI and type, with the read grant
/// (and the write grant for edit) also given through the `ClipData`.
fn view_intent<'a>(
    env: &mut JNIEnv<'a>,
    activity: &GlobalRef,
    path: &str,
    mime: &str,
    edit: bool,
) -> JniResult<JObject<'a>> {
    let uri = file_uri(env, activity, path)?;
    let action = env.new_string(if edit {
        "android.intent.action.EDIT"
    } else {
        "android.intent.action.VIEW"
    })?;
    let intent = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&action)],
    )?;
    let mime = env.new_string(mime)?;
    env.call_method(
        &intent,
        "setDataAndType",
        "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&uri), JValue::Object(&mime)],
    )?;
    let label = env.new_string("")?;
    let clip = env
        .call_static_method(
            "android/content/ClipData",
            "newRawUri",
            "(Ljava/lang/CharSequence;Landroid/net/Uri;)Landroid/content/ClipData;",
            &[JValue::Object(&label), JValue::Object(&uri)],
        )?
        .l()?;
    env.call_method(
        &intent,
        "setClipData",
        "(Landroid/content/ClipData;)V",
        &[JValue::Object(&clip)],
    )?;
    // FLAG_GRANT_READ_URI_PERMISSION = 1, FLAG_GRANT_WRITE_URI_PERMISSION = 2
    let flags = if edit { 1 | 2 } else { 1 };
    env.call_method(
        &intent,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(flags)],
    )?;
    Ok(intent)
}

/// `pm.queryIntentActivities(viewIntent, MATCH_DEFAULT_ONLY)`, without Kiln itself and without
/// repeated packages (the loop of `appChooser`).
fn apps_for(
    env: &mut JNIEnv<'_>,
    activity: &GlobalRef,
    path: &str,
    mime: &str,
    edit: bool,
) -> JniResult<Vec<OpenApp>> {
    let intent = view_intent(env, activity, path, mime, edit)?;
    let pm = env
        .call_method(
            activity,
            "getPackageManager",
            "()Landroid/content/pm/PackageManager;",
            &[],
        )?
        .l()?;
    let list = env
        .call_method(
            &pm,
            "queryIntentActivities",
            "(Landroid/content/Intent;I)Ljava/util/List;",
            &[JValue::Object(&intent), JValue::Int(0x10000)],
        )?
        .l()?;
    let own = package_name(env, activity)?;
    let count = env.call_method(&list, "size", "()I", &[])?.i()?;
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for i in 0..count {
        let item = env.with_local_frame(16, |env| -> JniResult<OpenApp> {
            let info = env
                .call_method(&list, "get", "(I)Ljava/lang/Object;", &[JValue::Int(i)])?
                .l()?;
            let activity_info = env
                .get_field(&info, "activityInfo", "Landroid/content/pm/ActivityInfo;")?
                .l()?;
            let package = env
                .get_field(&activity_info, "packageName", "Ljava/lang/String;")?
                .l()?;
            let package = char_seq(env, &package)?;
            let label = env
                .call_method(
                    &info,
                    "loadLabel",
                    "(Landroid/content/pm/PackageManager;)Ljava/lang/CharSequence;",
                    &[JValue::Object(&pm)],
                )?
                .l()?;
            let label = char_seq(env, &label)?;
            Ok(OpenApp { package, label })
        });
        let app = match item {
            Ok(app) => app,
            Err(_) => {
                clear_pending(env);
                continue;
            }
        };
        if app.package == own || !seen.insert(app.package.clone()) {
            continue;
        }
        apps.push(app);
    }
    Ok(apps)
}

/// `launch` (one app, `setPackage`) or the "More options..." row (`Intent.createChooser`).
/// An `ActivityNotFoundException` comes back as an error.
fn launch(
    env: &mut JNIEnv<'_>,
    activity: &GlobalRef,
    path: &str,
    mime: &str,
    edit: bool,
    with: &OpenWith,
) -> JniResult<()> {
    let intent = view_intent(env, activity, path, mime, edit)?;
    let intent = match with {
        OpenWith::Package(package) => {
            let package = env.new_string(package)?;
            env.call_method(
                &intent,
                "setPackage",
                "(Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue::Object(&package)],
            )?;
            intent
        }
        OpenWith::Chooser => {
            let title = env.new_string(if edit { "Edit with" } else { "Open with" })?;
            env.call_static_method(
                "android/content/Intent",
                "createChooser",
                "(Landroid/content/Intent;Ljava/lang/CharSequence;)Landroid/content/Intent;",
                &[JValue::Object(&intent), JValue::Object(&title)],
            )?
            .l()?
        }
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

    fn storage_root(&self) -> PathBuf {
        self.storage.clone()
    }

    fn utc_offset(&self, epoch_seconds: i64) -> i32 {
        self.with_env(|env, _| utc_offset(env, epoch_seconds))
            .unwrap_or(0)
    }

    fn volumes(&self) -> Vec<Volume> {
        self.with_env(removable_volumes).unwrap_or_default()
    }

    fn space(&self, path: &Path) -> Option<(u64, u64)> {
        let text = path.to_string_lossy().into_owned();
        self.with_env(|env, _| space_of(env, &text)).ok()
    }

    fn version_name(&self) -> String {
        self.version.clone()
    }

    fn version_code(&self) -> i64 {
        self.with_env(|env, activity| version_code(env, activity))
            .unwrap_or(0)
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

    fn set_clipboard(&self, text: &str) {
        let _ = self.with_env(|env, activity| put_clipboard(env, activity, text));
    }

    fn mime_for_extension(&self, ext: &str) -> Option<String> {
        self.with_env(|env, _| mime_of(env, ext)).ok().flatten()
    }

    fn open_apps(&self, path: &Path, mime: &str, edit: bool) -> Vec<OpenApp> {
        let text = path.to_string_lossy().into_owned();
        self.with_env(|env, activity| apps_for(env, activity, &text, mime, edit))
            .unwrap_or_default()
    }

    fn open_file(&self, path: &Path, mime: &str, edit: bool, with: &OpenWith) -> bool {
        let text = path.to_string_lossy().into_owned();
        self.with_env(|env, activity| launch(env, activity, &text, mime, edit, with))
            .is_ok()
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
