package app.kiln;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Outline;
import android.media.MediaMetadataRetriever;
import android.util.LruCache;
import android.view.View;
import android.view.ViewGroup;
import android.view.ViewOutlineProvider;
import android.widget.ImageView;

import java.io.File;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Icons by file type, plus small previews for pictures, music (cover art) and video (a frame).
 * Previews are decoded small, off the UI thread, and cached in memory.
 */
final class Thumbs {
    private Thumbs() {}

    static final int OTHER = 0, IMAGE = 1, AUDIO = 2, VIDEO = 3, CODE = 4;
    static final long IMAGE_MAX = 15_000_000L;

    private static final String IMAGE_EXT = " png jpg jpeg gif webp bmp heic heif ";
    private static final String AUDIO_EXT = " mp3 m4a flac ogg oga opus wav aac wma mka amr ";
    private static final String VIDEO_EXT = " mp4 mkv webm 3gp mov avi m4v ts ";
    private static final String CODE_EXT = " txt md json xml csv log py c h cpp java kt sh html css js "
            + "yml yaml ini conf cfg toml tsv rst srt gradle properties lua rb go rs ts sql "
            + "bash zsh fish bat cmd ps1 pl php swift dart jsx tsx vue cs scala mk cmake env "
            + "gitignore service desktop lock diff patch tex ";

    private static final ExecutorService POOL = Executors.newFixedThreadPool(2);
    private static final LruCache<String, Bitmap> CACHE = new LruCache<String, Bitmap>(
            (int) (Runtime.getRuntime().maxMemory() / 1024 / 10)) {
        @Override
        protected int sizeOf(String k, Bitmap b) { return b.getByteCount() / 1024; }
    };
    private static final Bitmap NONE = Bitmap.createBitmap(1, 1, Bitmap.Config.ALPHA_8);

    static int kindOf(String name) {
        String ext = " " + Fmt.ext(name) + " ";
        if (IMAGE_EXT.contains(ext)) return IMAGE;
        if (AUDIO_EXT.contains(ext)) return AUDIO;
        if (VIDEO_EXT.contains(ext)) return VIDEO;
        if (CODE_EXT.contains(ext)) return CODE;
        return OTHER;
    }

    /** True when a preview can be drawn for this kind (code and other files only get icons). */
    static boolean previewable(int kind) { return kind == IMAGE || kind == AUDIO || kind == VIDEO; }

    static int iconFor(String name) { return Icons.forFile(name); }

    /** A square, rounded icon tile. Used for list rows and the folder chooser. */
    static ImageView box(Context c) {
        ImageView iv = new ImageView(c);
        iv.setBackground(Ui.shape(c, c.getColor(R.color.k_chip), 0, 10));
        iv.setClipToOutline(true);
        iv.setOutlineProvider(new ViewOutlineProvider() {
            @Override
            public void getOutline(View v, Outline o) {
                o.setRoundRect(0, 0, v.getWidth(), v.getHeight(), Ui.dp(v.getContext(), 10));
            }
        });
        return iv;
    }

    /** Shows the plain icon at once, then swaps in a preview if there is one to show. */
    static void bind(final ImageView iv, final File f, int icon, int iconColor, boolean wantPreview) {
        bind(iv, f, icon, iconColor, wantPreview, false);
    }

    /** `bare`: the plain icon is drawn on its own, filling the box, with no tile behind it. */
    static void bind(final ImageView iv, final File f, int icon, int iconColor, boolean wantPreview, boolean bare) {
        final Context c = iv.getContext();
        final int kind = kindOf(f.getName());
        final String key = f.getPath() + "|" + f.lastModified();
        iv.setTag(key);
        showIcon(iv, icon, iconColor, bare);
        if (!wantPreview || !previewable(kind) || (kind == IMAGE && f.length() > IMAGE_MAX)) return;
        Bitmap hit = CACHE.get(key);
        if (hit != null) {
            if (hit != NONE) showBitmap(iv, hit);
            return;
        }
        final int px = Ui.dp(c, 96);
        POOL.execute(() -> {
            if (!key.equals(iv.getTag())) return;               // the row was reused meanwhile
            Bitmap b = null;
            try {
                b = load(f, kind, px);
            } catch (Throwable ignored) {
            }
            final Bitmap res = b;
            CACHE.put(key, res == null ? NONE : res);
            if (res != null) {
                iv.post(() -> {
                    if (key.equals(iv.getTag())) showBitmap(iv, res);
                });
            }
        });
    }

    static void showIcon(ImageView iv, int icon, int color) {
        showIcon(iv, icon, color, false);
    }

    static void showIcon(ImageView iv, int icon, int color, boolean bare) {
        iv.animate().cancel();
        iv.setAlpha(1f);
        iv.setScaleType(ImageView.ScaleType.FIT_CENTER);                    // scales up as well as down
        ViewGroup.LayoutParams lp = iv.getLayoutParams();
        int w = lp != null && lp.width > 0 ? lp.width : 0;
        int p;
        if (bare) {
            iv.setBackground(null);                                          // no tile: the artwork fills the box
            p = 0;
        } else {
            if (iv.getBackground() == null) iv.setBackground(Ui.shape(iv.getContext(), iv.getContext().getColor(R.color.k_chip), 0, 10));
            p = w > 0 ? Math.round(w * (Icons.tight(icon) ? 0.1f : 0.2f))   // folders fill the tile's width
                    : Ui.dp(iv.getContext(), 9);
        }
        iv.setPadding(p, p, p, p);
        if (bare && Icons.tight(icon)) {
            // a folder: the folder in its colour with a small white sheet showing at the top
            Context c = iv.getContext();
            android.graphics.drawable.Drawable base = c.getDrawable(icon).mutate();
            base.setTintList(android.content.res.ColorStateList.valueOf(color));
            android.graphics.drawable.Drawable paper = c.getDrawable(R.drawable.ic_paper);
            iv.setImageTintList(null);
            iv.setImageDrawable(new android.graphics.drawable.LayerDrawable(
                    new android.graphics.drawable.Drawable[]{base, paper}));
            return;
        }
        iv.setImageResource(icon);
        Ui.tint(iv, color);
    }

    private static void showBitmap(ImageView iv, Bitmap b) {
        if (iv.getBackground() == null) iv.setBackground(Ui.shape(iv.getContext(), iv.getContext().getColor(R.color.k_chip), 0, 10));
        iv.setImageTintList(null);
        iv.setPadding(0, 0, 0, 0);
        iv.setScaleType(ImageView.ScaleType.CENTER_CROP);
        iv.setImageBitmap(b);
        iv.setAlpha(0f);
        iv.animate().alpha(1f).setDuration(Defaults.anim(iv.getContext(), 160)).start();       // fades in when it arrives
    }

    private static Bitmap load(File f, int kind, int px) {
        if (kind == IMAGE) {
            BitmapFactory.Options o = new BitmapFactory.Options();
            o.inJustDecodeBounds = true;
            BitmapFactory.decodeFile(f.getPath(), o);
            int s = 1;
            while (o.outWidth / (s * 2) >= px && o.outHeight / (s * 2) >= px) s *= 2;
            o = new BitmapFactory.Options();
            o.inSampleSize = s;
            return BitmapFactory.decodeFile(f.getPath(), o);
        }
        MediaMetadataRetriever m = new MediaMetadataRetriever();
        try {
            m.setDataSource(f.getPath());
            if (kind == AUDIO) {
                byte[] art = m.getEmbeddedPicture();
                if (art == null) return null;
                BitmapFactory.Options o = new BitmapFactory.Options();
                o.inJustDecodeBounds = true;
                BitmapFactory.decodeByteArray(art, 0, art.length, o);
                int s = 1;
                while (o.outWidth / (s * 2) >= px && o.outHeight / (s * 2) >= px) s *= 2;
                o = new BitmapFactory.Options();
                o.inSampleSize = s;
                return BitmapFactory.decodeByteArray(art, 0, art.length, o);
            }
            return m.getScaledFrameAtTime(1_000_000L, MediaMetadataRetriever.OPTION_CLOSEST_SYNC, px, px);
        } finally {
            try {
                m.release();
            } catch (Throwable ignored) {
            }
        }
    }
}
