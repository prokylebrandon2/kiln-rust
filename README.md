# Kiln for Android

A front end for `kiln_v14.py`. The app bundles kiln (Python 3.12 through Chaquopy) and its C
helper, so it does not need Termux. It runs the same `list`, `test`, `unpack` and `pack`
commands and understands the same archives.

## Build (no PC needed)
1. Create a new GitHub repository and upload everything in this folder.
2. Open the **Actions** tab, choose **Build APK**, press **Run workflow**.
3. When it finishes, download the `kiln-apk` artifact, unzip it, install `app-debug.apk`
   (allow "install unknown apps" for your browser or file manager).
4. Open Kiln and allow **All files access** when asked.

## Build on a PC
Install Android Studio (JDK 17, Android SDK + NDK) and Python 3.12, then:

    python tools/build_accel.py app/src/main/python/kiln_v14.py \
        app/src/main/jniLibs/arm64-v8a/libkiln_accel.so \
        <NDK>/toolchains/llvm/prebuilt/<host>/bin/aarch64-linux-android30-clang
    gradle :app:assembleDebug

(repeat the first command for `x86_64` if you want an emulator build)

## Notes
* `app/src/main/python/kiln_v14.py` is your script plus one change: the environment variable
  `KILN_ACCEL_LIB` can name a prebuilt C helper, because an app has no C compiler.
* Passwords come from the app's own fields and are handed to kiln in memory. The app never
  puts them in an environment variable or a command line.
* Pack runs on one core (`-j 1`): worker processes are not safe inside an Android app.
* Needs Android 11 or newer. The app can only see shared storage, not Termux's private home.
