"""Start-up hook: tell kiln where the prebuilt C helper is, then load the app layer."""
import os


def init(helper_path, files_dir=""):
    os.environ["KILN_ACCEL_LIB"] = helper_path
    import kiln_scripts
    kiln_scripts.boot(files_dir)     # a custom main script replaces kiln_v14 before anything imports it
    import kiln_app          # imports kiln (which loads the helper) and the bridge
    import kiln_bridge
    kiln_app.fallback = kiln_scripts.load_fallback(files_dir)
    kiln_app.set_signatures(*kiln_scripts.signatures(files_dir))
    parts = []
    n = kiln_bridge.accel_note()
    if n:
        parts.append("Slow mode: " + n)
    if kiln_scripts.note():
        parts.append(kiln_scripts.note())
    return " ".join(parts)
