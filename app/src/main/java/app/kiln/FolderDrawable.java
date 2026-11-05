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
 * A folder drawn as one icon: a darker back panel with the tab, a sheet of paper showing between
 * the panels, and the lighter front panel on top. The shades come from the folder's colour, so every
 * colour gets the same look. The pictogram of the special folders is cut out of the front panel.
 * The artwork is on a 20 x 16 grid.
 */
final class FolderDrawable extends Drawable {
    private static final float W = 20f, H = 16f;
    private static final float FRONT_TOP = 4.4f;          // where the front panel starts

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
        back.draw(canvas);
        canvas.drawRoundRect(new RectF(b.left + 3f * sx, b.top + 2.9f * sy, b.left + 9.6f * sx, b.top + 4.3f * sy),
                0.6f * sy, 0.6f * sy, paper);
        canvas.save();
        canvas.clipRect(b.left, b.top + FRONT_TOP * sy, b.right, b.bottom);
        front.draw(canvas);
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
