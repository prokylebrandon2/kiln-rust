package app.kiln;

import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Locale;

/** Small formatting helpers. */
final class Fmt {
    private Fmt() {}

    static String size(long n) {
        if (n < 0) return "?";
        if (n < 1024) return n + " B";
        double v = n;
        String[] units = {"KB", "MB", "GB", "TB"};
        int u = -1;
        while (v >= 1024 && u < units.length - 1) {
            v /= 1024;
            u++;
        }
        return String.format(Locale.US, v < 10 ? "%.2f %s" : "%.1f %s", v, units[u]);
    }

    static String date(long epochSeconds) {
        if (epochSeconds <= 0) return "";
        return new SimpleDateFormat("yyyy-MM-dd HH:mm", Locale.getDefault())
                .format(new Date(epochSeconds * 1000L));
    }

    static String stripExt(String name) {
        int i = name.lastIndexOf('.');
        return i > 0 ? name.substring(0, i) : name;
    }

    static String ext(String name) {
        int i = name.lastIndexOf('.');
        return i >= 0 ? name.substring(i + 1).toLowerCase(Locale.ROOT) : "";
    }
}
