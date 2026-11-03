"""Start-up hook: tell kiln where the bundled native libraries are, then load the app layer."""
import os


def init(helper_path, files_dir="", rs_path=""):
    os.environ["KILN_ACCEL_LIB"] = helper_path
    import kiln_scripts
    kiln_scripts.clean_old(files_dir)
    import kiln_rs_engine
    kiln_rs_engine.configure(rs_path or None)
    import kiln_app          # imports kiln (which loads the helper) and the bridge
    import kiln_bridge
    parts = []
    n = kiln_bridge.accel_note()
    if n:
        parts.append("Slow mode: " + n)
    if kiln_rs_engine.note():
        parts.append(kiln_rs_engine.note())
    return " ".join(parts)
