"""Start-up housekeeping. The custom main script and the injectable engine were removed: the Rust
library is bundled with the app (libkiln_rs.so). This only deletes what older versions left behind."""
import os
import shutil


def clean_old(files_dir):
    """Remove the old scripts/ folder (custom main script, injected engine, scripts.json)."""
    if not files_dir:
        return
    d = os.path.join(files_dir, "scripts")
    if os.path.isdir(d):
        try:
            for n in os.listdir(d):
                p = os.path.join(d, n)
                try:
                    os.chmod(p, 0o600)
                except OSError:
                    pass
            shutil.rmtree(d, ignore_errors=True)
        except OSError:
            pass
