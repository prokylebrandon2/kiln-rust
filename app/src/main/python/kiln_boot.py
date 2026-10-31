"""Start-up hook: tell kiln where the prebuilt C helper is, then load the bridge."""
import os


def init(helper_path):
    os.environ["KILN_ACCEL_LIB"] = helper_path
    import kiln_bridge
    return kiln_bridge.accel_note()
