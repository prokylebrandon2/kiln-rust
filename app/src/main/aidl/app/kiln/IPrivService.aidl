package app.kiln;

// Runs shell commands inside the Shizuku server's process (the "shell" user, or root if Shizuku
// was started as root). Output goes to files so a big folder listing never hits the binder limit.
interface IPrivService {
    void destroy() = 16777114;              // reserved by Shizuku: called when the service is stopped
    int exec(String cmd, String outPath, String errPath) = 1;
    int uid() = 2;
}
