package app.kiln;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.IBinder;
import android.os.PowerManager;

/**
 * Keeps the process alive (and the CPU awake) while a long pack/unpack runs, and shows its
 * progress in a notification. The work itself runs in MainActivity's thread.
 */
public class OpService extends Service {
    private static final String CHANNEL = "kiln_ops";
    private static final int ID = 1;
    private static volatile String lastTitle = "Kiln";
    private PowerManager.WakeLock wakeLock;

    static void begin(Context c, String title) {
        lastTitle = title;
        try {
            c.startForegroundService(new Intent(c, OpService.class));
        } catch (Throwable ignored) {
            // the job still runs; it just isn't protected from being killed in the background
        }
    }

    static void end(Context c) {
        try {
            c.stopService(new Intent(c, OpService.class));
            NotificationManager nm = c.getSystemService(NotificationManager.class);
            if (nm != null) nm.cancel(ID);
        } catch (Throwable ignored) {
        }
    }

    static void progress(Context c, String text, int percent) {
        try {
            NotificationManager nm = c.getSystemService(NotificationManager.class);
            if (nm != null) nm.notify(ID, build(c, text, percent));
        } catch (Throwable ignored) {
        }
    }

    private static Notification build(Context c, String text, int percent) {
        Notification.Builder b = new Notification.Builder(c, CHANNEL)
                .setSmallIcon(R.drawable.ic_notify)
                .setContentTitle(lastTitle)
                .setContentText(text)
                .setOngoing(true)
                .setOnlyAlertOnce(true);
        if (percent >= 0) {
            b.setProgress(100, percent, false);
        } else {
            b.setProgress(0, 0, true);
        }
        Intent open = new Intent(c, MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        b.setContentIntent(PendingIntent.getActivity(c, 0, open,
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT));
        return b.build();
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        NotificationManager nm = getSystemService(NotificationManager.class);
        nm.createNotificationChannel(new NotificationChannel(
                CHANNEL, "Kiln jobs", NotificationManager.IMPORTANCE_LOW));
        startForeground(ID, build(this, "Working...", -1), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
        PowerManager pm = getSystemService(PowerManager.class);
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "kiln:job");
        wakeLock.acquire(2 * 60 * 60 * 1000L);
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        if (wakeLock != null && wakeLock.isHeld()) wakeLock.release();
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
