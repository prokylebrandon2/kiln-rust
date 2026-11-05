package app.kiln;

import android.content.Context;

import java.io.File;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;

/**
 * Which icon a file or folder gets. Files are told apart by extension (a distinct shape and colour
 * for documents, spreadsheets, PDFs, apps, archives, fonts, ...). Folders get a glyph when they are
 * one of Android's standard folders (Download, DCIM, Music, ...). Folder icons can also be picked
 * by key, so a person can choose one for any folder or bookmark.
 */
final class Icons {
    private Icons() {}

    // ------------------------------------------------------------------ files

    private static final int PDF = 0xFFE5574B, DOC = 0xFF4A8FE7, SHEET = 0xFF3FA66B, SLIDES = 0xFFE8833A,
            APK = 0xFF5DB75D, ZIP = 0xFFE0A030, FONT = 0xFF8E7CC3, DB = 0xFF5BA3B8, WEB = 0xFF3D9BE0,
            TERM = 0xFF6FBF73, KEY = 0xFFD6A53A, BOOK = 0xFFC77D4E, DISK = 0xFF8D99AE,
            CODE = 0xFF5B8DEF, IMAGE = 0xFF3FA6A0, AUDIO = 0xFFC45BA3, VIDEO = 0xFFE0634F;

    private static final class Look {
        final int icon, color;       // color 0 = the normal secondary text colour
        Look(int icon, int color) { this.icon = icon; this.color = color; }
    }

    private static final Map<String, Look> FILES = new HashMap<>();

    private static void add(String exts, int icon, int color) {
        Look l = new Look(icon, color);
        for (String e : exts.trim().split("\\s+")) FILES.put(e, l);
    }

    static {
        add("png jpg jpeg gif webp bmp heic heif svg tif tiff ico raw dng", R.drawable.ic_image, IMAGE);
        add("mp3 m4a flac ogg oga opus wav aac wma mka amr mid midi", R.drawable.ic_music, AUDIO);
        add("mp4 mkv webm 3gp mov avi m4v ts wmv flv mpg mpeg", R.drawable.ic_video, VIDEO);
        add("pdf", R.drawable.ic_f_pdf, PDF);
        add("doc docx odt rtf pages dot dotx", R.drawable.ic_f_doc, DOC);
        add("xls xlsx ods csv tsv numbers xlsm", R.drawable.ic_f_sheet, SHEET);
        add("ppt pptx odp key pps ppsx", R.drawable.ic_f_slides, SLIDES);
        add("apk xapk apks aab apkm", R.drawable.ic_f_apk, APK);
        add("zip 7z rar tar gz tgz bz2 xz zst lz4 cab jar war z lzma", R.drawable.ic_f_zip, ZIP);
        add("ttf otf woff woff2 ttc fon", R.drawable.ic_f_font, FONT);
        add("db sqlite sqlite3 sql mdb realm", R.drawable.ic_f_db, DB);
        add("html htm xhtml mhtml url webloc", R.drawable.ic_f_web, WEB);
        add("sh bash zsh fish bat cmd ps1", R.drawable.ic_f_term, TERM);
        add("pem crt cer der p12 pfx keystore jks gpg asc pub", R.drawable.ic_f_key, KEY);
        add("epub mobi azw azw3 fb2 cbz cbr djvu", R.drawable.ic_f_book, BOOK);
        add("iso img dmg vhd vmdk bin", R.drawable.ic_f_disk, DISK);
        add("exe dll so dex o class elf", R.drawable.ic_f_bin, DISK);
        add("txt log md rst srt vtt nfo", R.drawable.ic_f_text, 0);
        add("ini conf cfg toml yml yaml properties env gitignore service desktop lock", R.drawable.ic_settings, 0);
        add("json xml py c h cpp java kt gradle lua rb go rs sql js ts css pl php swift dart jsx tsx vue cs "
                + "scala mk cmake diff patch tex", R.drawable.ic_code, CODE);
    }

    /** Icon for a file name. */
    static int forFile(String name) {
        Look l = FILES.get(Fmt.ext(name));
        if (l != null) return l.icon;
        return R.drawable.ic_file;
    }

    /** Colour for a file's icon (the secondary text colour when it has no colour of its own). */
    static int tintFor(Context c, String name) {
        Look l = FILES.get(Fmt.ext(name));
        return l != null && l.color != 0 ? l.color : Ui.text2(c);
    }

    // ------------------------------------------------------------------ folders

    /** The picker's icons, in order: key, label, drawable. "folder" is the plain default. */
    static final String[][] FOLDER_KEYS = {
            {"folder", "Folder"}, {"download", "Download"}, {"camera", "Camera"}, {"pictures", "Pictures"},
            {"movies", "Movies"}, {"music", "Music"}, {"documents", "Documents"}, {"android", "Android"},
            {"bell", "Sounds"}, {"zip", "Archives"}, {"star", "Favourite"}, {"heart", "Heart"},
            {"lock", "Private"}, {"code", "Code"}, {"home", "Home"}, {"phone", "Phone storage"},
            {"sd", "SD card"}, {"usb", "USB drive"},
    };

    /** Drawable for an icon key (the plain folder for an unknown key). */
    /** Folder drawables whose artwork fills its whole box (no empty margin), so they get a thinner tile padding. */
    static boolean tight(int res) {
        switch (res) {
            case R.drawable.ic_d_android: case R.drawable.ic_d_bell: case R.drawable.ic_d_camera: case R.drawable.ic_d_code: case R.drawable.ic_d_documents: case R.drawable.ic_d_download: case R.drawable.ic_d_heart: case R.drawable.ic_d_lock: case R.drawable.ic_d_movies: case R.drawable.ic_d_music: case R.drawable.ic_d_pictures: case R.drawable.ic_d_star: case R.drawable.ic_d_zip: case R.drawable.ic_folder:
                return true;
            default:
                return false;
        }
    }

    static int byKey(String key) {
        if (key == null) return R.drawable.ic_folder;
        switch (key) {
            case "download": return R.drawable.ic_d_download;
            case "camera": return R.drawable.ic_d_camera;
            case "pictures": return R.drawable.ic_d_pictures;
            case "movies": return R.drawable.ic_d_movies;
            case "music": return R.drawable.ic_d_music;
            case "documents": return R.drawable.ic_d_documents;
            case "android": return R.drawable.ic_d_android;
            case "bell": return R.drawable.ic_d_bell;
            case "zip": return R.drawable.ic_d_zip;
            case "star": return R.drawable.ic_d_star;
            case "heart": return R.drawable.ic_d_heart;
            case "lock": return R.drawable.ic_d_lock;
            case "code": return R.drawable.ic_d_code;
            case "home": return R.drawable.ic_home;
            case "phone": return R.drawable.ic_d_phone;
            case "sd": return R.drawable.ic_d_sd;
            case "usb": return R.drawable.ic_usb;
            default: return R.drawable.ic_folder;
        }
    }

    /**
     * The icon key Android's standard folders get by default, or null for an ordinary folder.
     * `atStorageRoot`: the folder sits directly in the top of a storage, where these names are
     * Android's own. Camera and Screenshots live one level down (DCIM, Pictures).
     */
    static String defaultKey(File dir, boolean atStorageRoot) {
        String n = dir.getName().toLowerCase(Locale.ROOT);
        if (atStorageRoot) {
            switch (n) {
                case "download": case "downloads": return "download";
                case "dcim": return "camera";
                case "pictures": return "pictures";
                case "movies": case "dcim_video": return "movies";
                case "music": case "audiobooks": case "podcasts": case "recordings": return "music";
                case "documents": return "documents";
                case "android": return "android";
                case "alarms": case "notifications": case "ringtones": return "bell";
                default: return null;
            }
        }
        File p = dir.getParentFile();
        String pn = p == null ? "" : p.getName().toLowerCase(Locale.ROOT);
        if (n.equals("camera") && pn.equals("dcim")) return "camera";
        if (n.equals("screenshots") && (pn.equals("pictures") || pn.equals("dcim"))) return "pictures";
        return null;
    }

    /** Drawable for a folder with its standard icon (the plain folder if it has none). */
    static int forDir(File dir, boolean atStorageRoot) {
        return byKey(defaultKey(dir, atStorageRoot));
    }
}
