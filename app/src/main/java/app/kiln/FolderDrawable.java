package app.kiln;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.ColorFilter;
import android.graphics.Paint;
import android.graphics.PixelFormat;
import android.graphics.Path;
import android.graphics.PorterDuff;
import android.graphics.PorterDuffColorFilter;
import android.graphics.Rect;
import android.graphics.RectF;
import android.graphics.drawable.Drawable;

/**
 * A folder drawn as one icon: the folder in its colour, a darker shade on the tab and the strip under it
 * (the left part only), and a sheet of paper showing in that dark part. The shades come from the folder's colour, so every
 * colour gets the same look. The pictogram of the special folders is cut out of the front panel.
 * The artwork is on a 20 x 16 grid.
 */
final class FolderDrawable extends Drawable {
    private static final float W = 20f, H = 16f;

    private final Drawable back, front;
    private final Paint paper = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint arrowPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path arrow;

    FolderDrawable(Context c, int res, int color, boolean up) {
        int dark = shade(color, 0.72f);
        back = layer(c, res, dark);
        front = layer(c, res, color);
        paper.setColor(0xFFFFFFFF);
        arrowPaint.setColor(dark);
        if (up) {
            arrow = new Path();                            // an up arrow printed on the front panel
            arrow.moveTo(10f, 6.4f);
            arrow.lineTo(14.6f, 11f);
            arrow.lineTo(11.5f, 11f);
            arrow.lineTo(11.5f, 14.6f);
            arrow.lineTo(8.5f, 14.6f);
            arrow.lineTo(8.5f, 11f);
            arrow.lineTo(5.4f, 11f);
            arrow.close();
        } else {
            arrow = null;
        }
    }

    private static Drawable layer(Context c, int res, int color) {
        Drawable d = c.getDrawable(res).mutate();
        d.setTintList(null);                               // the vector's own tint would win over the filter
        d.setColorFilter(new PorterDuffColorFilter(color, PorterDuff.Mode.SRC_IN));
        return d;
    }

    static int shade(int color, float f) {
        int r = Math.round(((color >> 16) & 0xFF) * f);
        int g = Math.round(((color >> 8) & 0xFF) * f);
        int b = Math.round((color & 0xFF) * f);
        return 0xFF000000 | (r << 16) | (g << 8) | b;
    }

    @Override
    public void draw(Canvas canvas) {
        Rect b = getBounds();
        if (b.isEmpty()) return;
        float sx = b.width() / W, sy = b.height() / H;
        back.setBounds(b);
        front.setBounds(b);
        front.draw(canvas);                                // the whole folder in its own colour
        canvas.save();                                     // only the tab and the strip under it get the darker shade
        Path dark = new Path();
        dark.moveTo(b.left, b.top);
        dark.lineTo(b.left + 8f * sx, b.top);
        dark.lineTo(b.left + 11f * sx, b.top + 4.4f * sy);
        dark.lineTo(b.left, b.top + 4.4f * sy);
        dark.close();
        canvas.clipPath(dark);
        back.draw(canvas);
        canvas.drawRoundRect(new RectF(b.left + 2.4f * sx, b.top + 2.9f * sy, b.left + 8.6f * sx, b.top + 4.0f * sy),
                0.55f * sy, 0.55f * sy, paper);            // a sheet of paper in the dark part
        canvas.restore();
        if (arrow != null) {
            canvas.save();
            canvas.translate(b.left, b.top);
            canvas.scale(sx, sy);
            canvas.drawPath(arrow, arrowPaint);
            canvas.restore();
        }
    }

    @Override public int getIntrinsicWidth() { return front.getIntrinsicWidth(); }
    @Override public int getIntrinsicHeight() { return front.getIntrinsicHeight(); }
    @Override public void setAlpha(int alpha) { back.setAlpha(alpha); front.setAlpha(alpha); paper.setAlpha(alpha); arrowPaint.setAlpha(alpha); invalidateSelf(); }
    @Override public void setColorFilter(ColorFilter cf) { }
    @Override public int getOpacity() { return PixelFormat.TRANSLUCENT; }
}
