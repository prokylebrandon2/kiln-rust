package dev.kiln.app;

import android.app.Activity;
import android.os.Bundle;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.BufferedReader;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * Minimal shell around the Kiln command-line binary that CI packages as {@code libkiln.so}.
 *
 * It proves the binary runs on the device by executing a create / list / test / extract round
 * trip on a small generated directory and showing the output. It is a starting point for a real
 * UI, not a finished app.
 */
public class MainActivity extends Activity {
    private TextView output;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        output = new TextView(this);
        output.setTextIsSelectable(true);
        output.setTypeface(android.graphics.Typeface.MONOSPACE);
        output.setPadding(32, 48, 32, 32);
        output.setText("Running Kiln self-test...\n");
        ScrollView scroll = new ScrollView(this);
        scroll.addView(output);
        setContentView(scroll);

        new Thread(() -> {
            String report;
            try {
                report = selfTest();
            } catch (Exception e) {
                report = "Self-test crashed: " + e;
            }
            final String text = report;
            runOnUiThread(() -> output.setText(text));
        }).start();
    }

    private String selfTest() throws IOException, InterruptedException {
        String kiln = getApplicationInfo().nativeLibraryDir + "/libkiln.so";
        StringBuilder log = new StringBuilder();
        if (!new File(kiln).canExecute()) {
            return "libkiln.so is missing or not executable at " + kiln
                    + "\n(was the APK built with the CI workflow's cross-compile step?)";
        }

        File work = new File(getCacheDir(), "selftest");
        deleteRecursively(work);
        File src = new File(work, "src/sub");
        if (!src.mkdirs()) {
            throw new IOException("cannot create " + src);
        }
        StringBuilder sample = new StringBuilder();
        for (int i = 0; i < 2000; i++) {
            sample.append("Kiln on Android \u2014 \u00fcn\u00ef\u00e7\u00f6d\u00e9\n");
        }
        byte[] text = sample.toString().getBytes(StandardCharsets.UTF_8);
        write(new File(src, "text.txt"), text);
        write(new File(work, "src/empty.txt"), new byte[0]);

        File archive = new File(work, "test.kiln");
        File out = new File(work, "out");
        boolean ok = true;
        ok &= run(log, kiln, "--version");
        ok &= run(log, kiln, "create", archive.getPath(), new File(work, "src").getPath());
        ok &= run(log, kiln, "list", archive.getPath());
        ok &= run(log, kiln, "info", archive.getPath());
        ok &= run(log, kiln, "test", archive.getPath());
        ok &= run(log, kiln, "extract", archive.getPath(), "--output", out.getPath());

        // Paths are stored as given on the command line, so locate the restored copy by walking.
        File found = find(out, "text.txt");
        boolean same = found != null && Arrays.equals(readAll(found), text);
        log.append("\nRound trip byte-for-byte identical: ").append(same).append('\n');
        log.append(ok && same ? "\nPASS\n" : "\nFAIL\n");
        return log.toString();
    }

    /** Run a command, append "$ command" and its combined output to {@code log}. */
    private static boolean run(StringBuilder log, String... command)
            throws IOException, InterruptedException {
        List<String> shown = new ArrayList<>(Arrays.asList(command));
        shown.set(0, "kiln");
        log.append("$ ").append(String.join(" ", shown)).append('\n');
        Process p = new ProcessBuilder(command).redirectErrorStream(true).start();
        try (BufferedReader r = new BufferedReader(
                new InputStreamReader(p.getInputStream(), StandardCharsets.UTF_8))) {
            String line;
            while ((line = r.readLine()) != null) {
                log.append(line).append('\n');
            }
        }
        int code = p.waitFor();
        log.append("[exit ").append(code).append("]\n\n");
        return code == 0;
    }

    private static byte[] readAll(File file) throws IOException {
        try (FileInputStream in = new FileInputStream(file)) {
            ByteArrayOutputStream buf = new ByteArrayOutputStream();
            byte[] chunk = new byte[8192];
            int n;
            while ((n = in.read(chunk)) > 0) {
                buf.write(chunk, 0, n);
            }
            return buf.toByteArray();
        }
    }

    private static void write(File file, byte[] data) throws IOException {
        try (FileOutputStream f = new FileOutputStream(file)) {
            f.write(data);
        }
    }

    private static File find(File dir, String name) {
        File[] children = dir.listFiles();
        if (children == null) {
            return null;
        }
        for (File c : children) {
            if (c.isDirectory()) {
                File hit = find(c, name);
                if (hit != null) {
                    return hit;
                }
            } else if (c.getName().equals(name)) {
                return c;
            }
        }
        return null;
    }

    private static void deleteRecursively(File f) {
        File[] children = f.listFiles();
        if (children != null) {
            for (File c : children) {
                deleteRecursively(c);
            }
        }
        f.delete();
    }
}
