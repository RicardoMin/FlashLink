// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.app.AlertDialog;
import android.app.ProgressDialog;
import android.content.ClipData;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.provider.Settings;
import android.util.Log;

import androidx.core.content.FileProvider;

import java.io.File;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;

/** Owns update UI only while the device list/settings activity is in the foreground. */
final class FlashLinkUpdates {
    static final int ALLOW_SOURCE = 64;
    private static final String PERMISSION_STATE = "flashlink.update.pendingPermission";
    private final FlashLinkActivity activity;
    private final SharedPreferences prefs;
    private final PackageInfo installed;
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private Future<?> task;
    private boolean manual, checking, resumed;
    private int generation;
    private AlertDialog dialog;
    private Runnable pending;
    private byte[] awaitingPermission;

    FlashLinkUpdates(FlashLinkActivity activity, Bundle saved) {
        this.activity = activity;
        prefs = activity.getSharedPreferences("flashlink-updates", 0);
        try {
            installed = activity.getPackageManager().getPackageInfo(activity.getPackageName(), 0);
        } catch (PackageManager.NameNotFoundException e) {
            throw new IllegalStateException(e);
        }
        if (saved != null) awaitingPermission = saved.getByteArray(PERMISSION_STATE);
    }

    String version() { return installed.versionName; }

    void onResume() {
        resumed = true;
        if (!canShow()) return;
        if (pending != null) {
            Runnable action = pending;
            pending = null;
            action.run();
        }
        long now = System.currentTimeMillis(), previous = prefs.getLong("lastCheck", 0);
        if (task == null && dialog == null && awaitingPermission == null && (now < previous ||
                now - previous >= TimeUnit.DAYS.toMillis(1)) && official()) check(false);
    }

    private boolean official() { return FlashLinkAndroidRelease.PACKAGE.equals(installed.packageName); }

    void onPause() { resumed = false; }

    void save(Bundle state) { state.putByteArray(PERMISSION_STATE, awaitingPermission); }

    void destroy() {
        cancel();
        dismiss();
        worker.shutdownNow();
    }

    private boolean canShow() {
        return resumed && !activity.playerOpen() && !activity.isFinishing() && !activity.isDestroyed();
    }

    private void whenVisible(Runnable action) {
        if (canShow()) action.run();
        else pending = action;
    }

    private void dismiss() {
        if (dialog != null) {
            dialog.dismiss();
            dialog = null;
        }
    }

    private void cancel() {
        generation++;
        if (task != null) task.cancel(true);
        task = null;
        checking = false;
        pending = null;
    }

    void check(boolean requested) {
        if (!canShow()) return;
        if (!official()) {
            message("开发版本", "当前版本 " + version() + "。应用内更新适用于 FlashLink 正式版。");
            return;
        }
        if (task != null) {
            if (checking && requested && !manual) {
                manual = true;
                showProgress(false);
            }
            return;
        }
        manual = requested;
        checking = true;
        if (requested) showProgress(false);
        prefs.edit().putLong("lastCheck", System.currentTimeMillis()).apply();
        int token = ++generation;
        task = worker.submit(() -> {
            try {
                FlashLinkAndroidRelease release = FlashLinkAndroidRelease.check(activity);
                boolean available = release.newerThan(installed);
                finish(token, () -> {
                    if (available) whenVisible(() -> offer(release));
                    else if (manual) whenVisible(() -> message("已是最新版本", "当前版本 " + version()));
                });
            } catch (Exception e) {
                Log.w("FlashLinkUpdate", "Check failed", e);
                finish(token, () -> {
                    if (manual) whenVisible(() -> message("检查更新失败", "无法获取或验证版本信息。请检查网络后重试。"));
                });
            }
        });
    }

    private void finish(int token, Runnable result) {
        activity.runOnUiThread(() -> {
            if (generation != token || activity.isDestroyed() || activity.isFinishing()) return;
            task = null;
            checking = false;
            dismiss();
            result.run();
        });
    }

    private void offer(FlashLinkAndroidRelease release) {
        show(new AlertDialog.Builder(activity).setTitle("FlashLink 有新版本")
                .setMessage(version() + " → " + release.version + "\n\n" + release.notes)
                .setNegativeButton("稍后", null)
                .setPositiveButton("下载更新", (d, w) -> download(release, false)));
    }

    private void download(FlashLinkAndroidRelease release, boolean cachedOnly) {
        showProgress(true);
        int token = ++generation;
        task = worker.submit(() -> {
            try {
                File apk = release.prepareApk(activity, cachedOnly, percent -> activity.runOnUiThread(() -> {
                    if (generation == token && dialog instanceof ProgressDialog)
                        ((ProgressDialog) dialog).setProgress(percent);
                }));
                finish(token, () -> whenVisible(() -> install(release, apk)));
            } catch (Exception e) {
                Log.w("FlashLinkUpdate", "Package verification or download failed", e);
                finish(token, () -> whenVisible(() -> message("更新失败", "安装包未能完整下载或通过校验，请重新检查更新。")));
            }
        });
    }

    private void install(FlashLinkAndroidRelease release, File apk) {
        try {
            if (Build.VERSION.SDK_INT >= 26 && !activity.getPackageManager().canRequestPackageInstalls()) {
                show(new AlertDialog.Builder(activity).setTitle("允许安装 FlashLink 更新")
                        .setMessage("请在系统设置中允许来自 FlashLink 的安装请求，然后返回确认更新。")
                        .setNegativeButton("取消", null).setPositiveButton("打开设置", (d, w) -> {
                            try {
                                awaitingPermission = release.envelope;
                                activity.startActivityForResult(new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                                        Uri.parse("package:" + activity.getPackageName())), ALLOW_SOURCE);
                            } catch (RuntimeException e) {
                                awaitingPermission = null;
                                message("无法打开设置", "请在系统设置中为 FlashLink 开启安装应用权限。");
                            }
                        }));
                return;
            }
            Uri uri = FileProvider.getUriForFile(activity, activity.getPackageName() + ".updates", apk);
            Intent intent = new Intent(Intent.ACTION_INSTALL_PACKAGE, uri);
            intent.setClipData(ClipData.newRawUri("FlashLink", uri));
            intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            activity.startActivity(intent);
        } catch (RuntimeException e) {
            message("无法启动安装器", "请检查系统的应用安装设置后重试。");
        }
    }

    void permissionResult() {
        byte[] envelope = awaitingPermission;
        awaitingPermission = null;
        if (envelope == null) return;
        whenVisible(() -> {
            if (Build.VERSION.SDK_INT >= 26 && !activity.getPackageManager().canRequestPackageInstalls()) {
                message("尚未安装更新", "你可以稍后在「设置」中重新检查更新。");
                return;
            }
            try {
                // The OS may recreate this Activity while the user grants permission.
                download(FlashLinkAndroidRelease.parse(activity, envelope), true);
            } catch (Exception e) {
                message("无法继续安装", "请重新检查更新。");
            }
        });
    }

    private void showProgress(boolean downloading) {
        dismiss();
        ProgressDialog progress = new ProgressDialog(activity);
        progress.setTitle(downloading ? "正在准备更新" : "正在检查更新");
        progress.setMessage(downloading ? "正在下载并验证安装包…" : "当前版本 " + version());
        if (downloading) progress.setProgressStyle(ProgressDialog.STYLE_HORIZONTAL);
        progress.setCancelable(true);
        progress.setButton(ProgressDialog.BUTTON_NEGATIVE, "取消", (d, w) -> cancel());
        progress.setOnCancelListener(d -> cancel());
        remember(progress);
        progress.show();
    }

    private void message(String title, String text) {
        show(new AlertDialog.Builder(activity).setTitle(title).setMessage(text).setPositiveButton("知道了", null));
    }

    private void show(AlertDialog.Builder builder) {
        dismiss();
        AlertDialog next = builder.create();
        remember(next);
        next.show();
    }

    private void remember(AlertDialog shown) {
        dialog = shown;
        shown.setOnDismissListener(d -> { if (dialog == shown) dialog = null; });
    }
}
