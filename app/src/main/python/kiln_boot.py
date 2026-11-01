"""Start-up hook: tell kiln where the prebuilt C helper is, then load the app layer."""
import os


def init(helper_path):
    os.environ["KILN_ACCEL_LIB"] = helper_path
    import kiln_app          # imports kiln (which loads the helper) and the bridge
    import kiln_bridge
    return kiln_bridge.accel_note()
