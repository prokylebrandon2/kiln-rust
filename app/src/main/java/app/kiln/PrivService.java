package app.kiln;

import java.io.IOException;

/**
 * The Shizuku "user service": Shizuku starts this class in its own process (shell or root user),
 * and Kiln calls exec() on it through the IPrivService binder. It never runs inside the app.
 */
public class PrivService extends IPrivService.Stub {
    public PrivService() { }

    @Override
    public void destroy() {
        System.exit(0);
    }

    private static String q(String s) {
        return "'" + s.replace("'", "'\\''") + "'";
    }

    @Override
    public int exec(String cmd, String outPath, String errPath) {
        try {
            // the newline before "}" lets a command end in a comment without eating the redirect
            String script = "{ " + cmd + "\n} >" + q(outPath) + " 2>" + q(errPath);
            Process p = new ProcessBuilder("sh", "-c", script).start();
            try {
                p.getOutputStream().close();                // nothing reads stdin: never wait on it
            } catch (IOException ignored) {
            }
            return p.waitFor();
        } catch (Exception e) {
            return -1;
        }
    }

    @Override
    public int uid() {
        return android.os.Process.myUid();
    }
}
