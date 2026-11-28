// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.content.Intent;
import android.content.res.Configuration;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.SystemClock;
import android.preference.PreferenceManager;
import android.util.Log;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;
import android.widget.Toast;

import com.limelight.binding.audio.AndroidAudioRenderer;
import com.limelight.nvstream.NvConnection;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Thin shell around Moonlight's real Activity and its native callbacks. */
public final class FlashLinkGame extends Game {
    static final String DEADLINE = "dev.flashlink.recoveryDeadline";
    static final int NETWORK_LOST = RESULT_FIRST_USER + 1;
    static final int EXPIRED = RESULT_FIRST_USER + 2;
    static final int QUALITY_CHANGE = RESULT_FIRST_USER + 3;
    static final int QUALITY_FAILED = RESULT_FIRST_USER + 4;
    static final String PREVIOUS_QUALITY = "dev.flashlink.previousQuality";
    static final String FAILURE = "dev.flashlink.qualityFailure";
    static final String RESTORE_FAILURE = "dev.flashlink.restoreFailure";
    static final String RESTORING = "dev.flashlink.restoringQuality";
    static final String RESTORE_UNTIL = "dev.flashlink.restoreUntil";
    private static final String RESTORE_SIZE_PREF = "dev.flashlink.restoreSizePreference";
    static final String RETRYABLE = "dev.flashlink.retryable";
    static final String LOG = "FlashLinkSession";
    private final Handler timer = new Handler();
    private final Runnable expire = () -> endWith(EXPIRED, 0);
    private long recoveryDeadline;
    private long startedAt;
    private boolean foreground;
    private FlashLinkStreamTools tools;
    private FlashLinkStreamTouch streamTouch;
    private FlashLinkPrivacyLease privacyLease;
    private AndroidAudioRenderer audioRenderer;
    private boolean mediaRequested;
    private SurfaceHolder pendingSurface;
    private Runnable startMedia;
    private FlashLinkQuality activeQuality;
    private volatile FlashLinkQuality pendingQuality;
    private volatile boolean handingOff;
    private volatile String negotiationProblem;
    private TextView qualityProgress;
    private final Runnable qualityTimeout = () -> qualityFailed("等待电脑连接并传回第一帧画面超时。", true);

    private final ExecutorService settingSender = Executors.newSingleThreadExecutor();
    private final FlashLinkLiveSetting liveBitrate = newSetting(FlashLinkLiveSetting.Kind.BITRATE);
    private final FlashLinkLiveSetting liveFps = newSetting(FlashLinkLiveSetting.Kind.FRAME_RATE);
    private final FlashLinkLiveSetting liveSize = newSetting(FlashLinkLiveSetting.Kind.RESOLUTION);
    private final FlashLinkResolution resolution = new FlashLinkResolution(new FlashLinkResolution.Listener() {
        @Override public boolean request(int size) {
            setDecoderResizing(true);
            if (!liveSize.set(size)) {
                setDecoderResizing(false);
                return false;
            }
            scheduleResolutionTimeout();
            return true;
        }

        @Override public void finished(int size, boolean saved) {
            setDecoderResizing(false);
            int width = size >>> 16, height = size & 0xffff;
            if (saved) {
                PreferenceConfiguration.saveStreamSize(FlashLinkGame.this, width, height);
                Log.i(LOG, "RESOLUTION_APPLIED " + width + "x" + height);
                notifySetting("分辨率已切换至 " + width + " × " + height);
            } else {
                notifySetting("未应用新分辨率，继续使用原画面。");
            }
        }

        @Override public void reconnect(int previous, boolean restorePreference) {
            FlashLinkQuality restore = new FlashLinkQuality(previous >>> 16, previous & 0xffff,
                    activeQuality.fps, activeQuality.bitrateKbps);
            Intent data = new Intent(getIntent()).putExtra(PREVIOUS_QUALITY, restore.toArray())
                    .putExtra(RESTORE_SIZE_PREF, restorePreference)
                    .putExtra(FAILURE, "新分辨率未能显示，正在恢复上次可用画质。");
            data.removeExtra(RESTORING);
            handOffQuality(QUALITY_FAILED, data);
        }
    });
    private boolean openQualityWhenReady;

    private void scheduleResolutionTimeout() {
        int token = resolution.generation();
        timer.postDelayed(() -> resolution.timeout(token), 6500);
    }

    private FlashLinkLiveSetting newSetting(FlashLinkLiveSetting.Kind kind) {
        boolean fps = kind == FlashLinkLiveSetting.Kind.FRAME_RATE;
        boolean size = kind == FlashLinkLiveSetting.Kind.RESOLUTION;
        return new FlashLinkLiveSetting(kind, new FlashLinkLiveSetting.Listener() {
            @Override public void send(int id, int value) {
                FlashLinkLiveSetting setting = size ? liveSize : fps ? liveFps : liveBitrate;
                settingSender.execute(() -> {
                    boolean queued;
                    if (size || fps) {
                        queued = frameRate.sendRequest(value, () -> size
                                ? MoonBridge.requestResolution(FlashLinkGame.this, id, value)
                                : MoonBridge.requestFrameRate(FlashLinkGame.this, id, value));
                    } else {
                        queued = MoonBridge.requestBitrate(FlashLinkGame.this, id, value);
                    }
                    if (!queued) timer.post(() -> setting.timeout(id));
                });
                timer.postDelayed(() -> setting.timeout(id), 2000);
            }

            @Override public void result(FlashLinkLiveSetting.Result result, int value) {
                if (size) {
                    resolution.response(result, value);
                    // During rollback the last visible frame can still have the old size.
                    // Ask the renderer to report another frame even if its format is unchanged.
                    if (resolution.pending() && result == FlashLinkLiveSetting.Result.APPLIED)
                        confirmNextOutputAt(value);
                } else onSettingResult(fps, result, value);
                if (openQualityWhenReady && !waitingForSetting()) {
                    openQualityWhenReady = false;
                    if (foreground && !handingOff && !isFinishing() && !isDestroyed()) openQualityDialog();
                }
            }
        });
    }

    private void onSettingResult(boolean fps, FlashLinkLiveSetting.Result result, int value) {
        String label = fps ? "帧率" : "目标码率";
        boolean applied = result == FlashLinkLiveSetting.Result.APPLIED;
        if (value > 0 && activeQuality != null) {
            if (fps) {
                // An initial query often reports 59 while the user's saved target is 60.
                if (applied || activeQuality.fps != value) updateStreamFrameRate(value, applied);
                activeQuality = new FlashLinkQuality(activeQuality.width, activeQuality.height,
                        value, activeQuality.bitrateKbps);
            } else {
                updateStreamBitrate(value, applied);
                activeQuality = new FlashLinkQuality(activeQuality.width, activeQuality.height, activeQuality.fps, value);
            }
        }
        switch (result) {
            case APPLIED:
                Log.i(LOG, (fps ? "FPS_APPLIED " : "BITRATE_APPLIED ") + value);
                notifySetting(label + "已切换至 " + (fps ? value + " FPS" : FlashLinkQuality.mbps(value) + " Mbps"));
                break;
            case REJECTED:
                notifySetting("电脑未应用所选" + label + "，串流继续。");
                break;
            case UNCONFIRMED:
                notifySetting("尚未确认电脑当前" + label + "，串流继续。");
                break;
            case CURRENT:
                break;
        }
    }

    private boolean waitingForSetting() {
        return liveBitrate.waiting() || liveFps.waiting() || liveSize.waiting() || resolution.pending();
    }

    private void notifySetting(String message) {
        if (foreground && !isFinishing()) Toast.makeText(this, message, Toast.LENGTH_SHORT).show();
    }

    @Override public void bitrateResult(int id, int status, int kbps) {
        timer.post(() -> liveBitrate.response(id, status, kbps));
    }
    @Override public void frameRateResult(int id, int status, int fps) {
        timer.post(() -> liveFps.response(id, status, fps));
    }
    @Override public void resolutionResult(int id, int status, int size) {
        timer.post(() -> liveSize.response(id, status, size));
    }

    void showQualitySettings() {
        if (liveBitrate.state() == FlashLinkLiveSetting.State.CLOSED) return;
        if (liveBitrate.probing() || liveFps.probing() || liveSize.probing()) {
            openQualityWhenReady = true;
            liveBitrate.probe();
            liveFps.probe();
            liveSize.probe();
        } else if (waitingForSetting()) {
            notifySetting("正在确认串流设置…");
        } else {
            openQualityDialog();
        }
    }

    private void openQualityDialog() {
        new FlashLinkQualityDialog(this, activeQuality, liveBitrate.state(), liveFps.state(), liveSize.state()).show();
    }

    private void closeLiveSettings() {
        openQualityWhenReady = false;
        liveBitrate.close();
        liveFps.close();
        liveSize.close();
        resolution.close();
        settingSender.shutdownNow();
    }

    @Override protected void onCreate(Bundle state) {
        pendingQuality = FlashLinkQuality.fromArray(getIntent().getIntArrayExtra(FlashLinkQuality.EXTRA));
        super.onCreate(state);
        recoveryDeadline = getIntent().getLongExtra(DEADLINE, 0);
        if (recoveryDeadline > 0) {
            timer.postDelayed(expire, Math.max(0, recoveryDeadline - SystemClock.elapsedRealtime()));
        }
        if (pendingQuality != null && !isFinishing()) {
            showQualityProgress(isRestoring() ? "正在恢复上次画质…" : "正在应用画质，等待电脑画面…");
            long timeout = isRestoring() ? getIntent().getLongExtra(RESTORE_UNTIL, 0) - SystemClock.elapsedRealtime()
                    : FlashLinkReconnect.WINDOW_MS;
            timer.postDelayed(qualityTimeout, Math.max(0, timeout));
        }
    }

    @Override public void connectionStarted() {
        super.connectionStarted();
        runOnUiThread(() -> {
            if (handingOff || isFinishing() || isDestroyed()) return;
            startedAt = SystemClock.elapsedRealtime();
            if (recoveryDeadline > 0 && startedAt >= recoveryDeadline) {
                expire.run();
                return;
            }
            timer.removeCallbacks(expire);
            Log.i(LOG, recoveryDeadline == 0 ? "STREAM_STARTED" : "STREAM_RESUMED");
            if (privacyLease == null) privacyLease = startLease(null);
            else privacyLease.connected();
            if (!managedAudio() && recoveryDeadline == 0 && pendingQuality == null) {
                Toast.makeText(this, "本次使用兼容音频模式，断开后的电脑音量设置不会生效。", Toast.LENGTH_LONG).show();
            }
            streamReady();
        });
    }

    @Override protected void onStreamFrameReady(FlashLinkQuality active) {
        activeQuality = active;
        streamReady();
    }

    @Override protected void onStreamSizePresented(int width, int height) {
        if (handingOff || activeQuality == null || isFinishing()) return;
        if (activeQuality.width != width || activeQuality.height != height) {
            updateStreamSize(width, height);
            activeQuality = new FlashLinkQuality(width, height, activeQuality.fps, activeQuality.bitrateKbps);
        }
        Log.i(LOG, "RESOLUTION_PRESENTED " + width + "x" + height);
        resolution.presented(FlashLinkResolution.pack(width, height));
    }

    @Override protected void recoverOversizedStream(int size) {
        resolution.inputTooSmall(size);
    }

    private void streamReady() {
        if (startedAt == 0 || activeQuality == null || tools != null || handingOff || isFinishing() || isDestroyed()) return;
        if (pendingQuality != null) {
            if (isRestoring() && SystemClock.elapsedRealtime() >= getIntent().getLongExtra(RESTORE_UNTIL, 0)) {
                qualityTimeout.run();
                return;
            }
            if (!pendingQuality.equals(activeQuality)) {
                qualityFailed("电脑协商的参数为 " + activeQuality.displayText() + "，与所选画质不一致。", false);
                return;
            }
            timer.removeCallbacks(qualityTimeout);
            if (isRestoring()) {
                if (getIntent().getBooleanExtra(RESTORE_SIZE_PREF, false)) {
                    PreferenceConfiguration.saveStreamSize(this, activeQuality.width, activeQuality.height);
                    getIntent().removeExtra(RESTORE_SIZE_PREF);
                }
                Toast.makeText(this, lastQualityFailure() + "\n已恢复上次画质。", Toast.LENGTH_LONG).show();
            } else {
                rememberStreamQuality(activeQuality);
                Toast.makeText(this, "画质已切换至 " + activeQuality.displayText(), Toast.LENGTH_SHORT).show();
            }
            Log.i(LOG, (isRestoring() ? "QUALITY_RESTORED " : "QUALITY_APPLIED ") + activeQuality.displayText());
            pendingQuality = null;
            qualityProgress.setVisibility(View.GONE);
        }
        tools = new FlashLinkStreamTools(this);
        tools.attach();
        liveBitrate.probe();
        liveFps.probe();
        liveSize.probe();
    }

    FlashLinkQuality currentQuality() { return activeQuality; }
    String lastQualityFailure() { return isRestoring() ? getIntent().getStringExtra(FAILURE) : null; }

    void applyQuality(FlashLinkQuality selected, boolean reconnect) {
        if (tools == null || activeQuality == null || selected.equals(activeQuality) || handingOff) return;
        if (!reconnect) {
            boolean queued;
            if (selected.width != activeQuality.width || selected.height != activeQuality.height) {
                queued = resolution.begin(FlashLinkResolution.pack(activeQuality.width, activeQuality.height),
                        FlashLinkResolution.pack(selected.width, selected.height));
            } else {
                queued = selected.fps != activeQuality.fps ? liveFps.set(selected.fps) : liveBitrate.set(selected.bitrateKbps);
            }
            if (!queued) notifySetting("暂时无法切换串流设置，请重试。");
            return;
        }
        Intent next = new Intent(getIntent());
        next.putExtra(FlashLinkQuality.EXTRA, selected.toArray());
        next.putExtra(PREVIOUS_QUALITY, activeQuality.toArray());
        next.removeExtra(RESTORING);
        next.removeExtra(FAILURE);
        next.removeExtra(RESTORE_FAILURE);
        next.removeExtra(DEADLINE);
        showQualityProgress("正在切换到 " + selected.displayText() + "…");
        handOffQuality(QUALITY_CHANGE, next);
    }

    private boolean isRestoring() { return getIntent().getBooleanExtra(RESTORING, false); }

    private void showQualityProgress(String message) {
        if (qualityProgress == null) {
            qualityProgress = new TextView(this);
            int inset = Math.round(12 * getResources().getDisplayMetrics().density);
            qualityProgress.setPadding(inset, inset, inset, inset);
            qualityProgress.setTextColor(android.graphics.Color.WHITE);
            qualityProgress.setBackgroundColor(0xdf202833);
            addContentView(qualityProgress, new FrameLayout.LayoutParams(-2, -2, Gravity.CENTER));
        }
        qualityProgress.setText(message);
        qualityProgress.setVisibility(View.VISIBLE);
    }

    private void qualityFailed(String reason, boolean retryable) {
        if (pendingQuality == null || handingOff || isFinishing() || isDestroyed()) return;
        showQualityProgress(reason);
        Log.i(LOG, "QUALITY_FAILED");
        handOffQuality(QUALITY_FAILED, new Intent(getIntent()).putExtra(isRestoring() ? RESTORE_FAILURE : FAILURE, reason)
                .putExtra(RETRYABLE, retryable));
    }

    private void handOffQuality(int result, Intent next) {
        handingOff = true;
        closeLiveSettings();
        if (privacyLease != null) {
            privacyLease.handoff();
            privacyLease = null;
        }
        timer.removeCallbacks(qualityTimeout);
        releaseControls();
        if (tools != null) tools.setVisible(false);
        // The native stop is asynchronous. Wait for teardown before another player starts /resume.
        stopConnectionForQuality(() -> {
            if (isFinishing() || isDestroyed()) return;
            setResult(result, next);
            finish();
        });
    }

    @Override protected String stageProgressMessage(String stage) {
        if (pendingQuality != null) {
            return isRestoring() ? "正在恢复上次画质…" : "正在协商新的串流画质…";
        }
        return super.stageProgressMessage(stage);
    }

    @Override public void displayMessage(String message) {
        if (pendingQuality == null) super.displayMessage(message);
        else negotiationProblem = message;
    }

    @Override public void connectionTerminated(int code) {
        runOnUiThread(() -> {
            if (handingOff || isFinishing() || isDestroyed()) return;
            releaseControls();
            closeLiveSettings();
            // Decide alongside the first-frame callback: it may already have committed this profile.
            if (pendingQuality != null) {
                qualityFailed(code == MoonBridge.ML_ERROR_NO_VIDEO_FRAME || code == MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC
                        ? "电脑未能传回可播放的视频画面。" : "画质切换期间连接中断（" + code + "）。",
                        FlashLinkReconnect.transportLost(code) || code == MoonBridge.ML_ERROR_NO_VIDEO_FRAME);
                return;
            }
            closeLease(!FlashLinkReconnect.transportLost(code));
            if (!FlashLinkReconnect.transportLost(code) || (startedAt == 0 && recoveryDeadline == 0)) {
                timer.removeCallbacks(expire);
                // Upstream collects connectivity diagnostics before displaying its error screen.
                new Thread(() -> super.connectionTerminated(code), "FlashLink stream error").start();
                return;
            }
            if (!foreground) return;
            long now = SystemClock.elapsedRealtime();
            long deadline = recoveryDeadline;
            if (deadline == 0 || (startedAt > 0 && now - startedAt >= FlashLinkReconnect.WINDOW_MS)) {
                deadline = now + FlashLinkReconnect.WINDOW_MS;
            }
            Log.i(LOG, "STREAM_INTERRUPTED");
            endWith(NETWORK_LOST, deadline);
        });
    }

    @Override public void stageFailed(String stage, int ports, int error) {
        if (handingOff || isFinishing()) return;
        runOnUiThread(() -> { if (startedAt == 0) closeLease(false); });
        if (pendingQuality != null) {
            String detail = negotiationProblem;
            String reason = stage + " 阶段失败" + (error != 0 ? "（" + error + "）" : "") + "。";
            boolean retryable = (ports != 0 && (error == 0 || FlashLinkReconnect.transportLost(error)))
                    || error == 503;
            runOnUiThread(() -> qualityFailed(detail == null || detail.isEmpty() ? reason : reason + detail, retryable));
        } else if (recoveryDeadline > 0 && ports != 0 && (error == 0 || FlashLinkReconnect.transportLost(error))) {
            runOnUiThread(() -> { if (foreground) endWith(NETWORK_LOST, recoveryDeadline); });
        } else {
            timer.removeCallbacks(expire);
            super.stageFailed(stage, ports, error);
        }
    }

    private void endWith(int result, long deadline) {
        if (isFinishing() || isDestroyed()) return;
        releaseControls();
        setResult(result, new Intent().putExtra(DEADLINE, deadline));
        finish();
    }

    @Override protected boolean handleStreamFingerTouch(View source, View video, MotionEvent event,
                                                        NvConnection connection) {
        if (tools == null || handingOff || !foreground || !hasWindowFocus() || isFinishing()) return true;
        if (streamTouch == null) streamTouch = new FlashLinkStreamTouch(this, video, connection);
        return streamTouch.onTouch(source, event, isTouchscreenTrackpad());
    }

    @Override protected void onTouchscreenCancelled() {
        if (streamTouch != null) streamTouch.cancel();
    }

    void showViewReset(boolean zoomed) {
        if (tools != null) tools.showViewReset(zoomed);
    }

    void resetViewport() {
        releaseControls();
        if (streamTouch != null) streamTouch.reset();
    }

    void toggleTouchMode() {
        if (tools != null) tools.release();
        boolean trackpad = !isTouchscreenTrackpad();
        setTouchscreenTrackpad(trackpad);
        PreferenceManager.getDefaultSharedPreferences(this).edit()
                .putBoolean(PreferenceConfiguration.TOUCHSCREEN_TRACKPAD_PREF_STRING, trackpad).apply();
    }

    private boolean managedAudio() {
        return getIntent().getBooleanExtra(FlashLinkAudio.MANAGED, false);
    }

    private FlashLinkPrivacyLease startLease(FlashLinkPrivacyLease.Preflight preflight) {
        return FlashLinkPrivacyLease.start(this, getIntent().getStringExtra(EXTRA_HOST),
                getIntent().getIntExtra(EXTRA_PORT, com.limelight.nvstream.http.NvHTTP.DEFAULT_HTTP_PORT),
                getIntent().getByteArrayExtra(EXTRA_SERVER_CERT), managedAudio(), preflight);
    }

    @Override public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        boolean needsMute = getIntent().getBooleanExtra(FlashLinkAudio.PREFLIGHT, false)
                && FlashLinkAudio.CONTROLLER.equals(FlashLinkAudio.route(this));
        if (mediaRequested || (pendingSurface == null && !needsMute)) {
            mediaRequested = true;
            super.surfaceChanged(holder, format, width, height);
            return;
        }
        pendingSurface = holder;
        startMedia = () -> super.surfaceChanged(holder, format, width, height);
        prepareSurface();
    }

    private void prepareSurface() {
        if (!foreground || pendingSurface == null || privacyLease != null) return;
        privacyLease = startLease((lease, muted) -> runOnUiThread(() -> {
            if (privacyLease != lease || handingOff || isFinishing() || isDestroyed()) return;
            if (!muted) {
                closeLease(false);
                String reason = "电脑未确认扬声器已静音。请检查电脑的播放设备和局域网后重新连接。";
                if (pendingQuality != null) qualityFailed(reason, true);
                else {
                    Toast.makeText(this, reason, Toast.LENGTH_LONG).show();
                    finish();
                }
                return;
            }
            // A queued confirmation may outlive its Surface or the host's preparation lease.
            if (mediaRequested || !foreground || pendingSurface == null || !pendingSurface.getSurface().isValid()
                    || !lease.canStart()) return;
            mediaRequested = true;
            pendingSurface = null;
            Runnable start = startMedia;
            startMedia = null;
            start.run();
        }));
    }

    @Override public void surfaceDestroyed(SurfaceHolder holder) {
        if (pendingSurface == holder) {
            pendingSurface = null;
            startMedia = null;
            closeLease(false);
        }
        super.surfaceDestroyed(holder);
    }

    @Override protected boolean playAudioOnHost() {
        // The upgraded host captures its ordinary output and controls the physical speaker itself.
        return managedAudio() || !FlashLinkAudio.CONTROLLER.equals(FlashLinkAudio.route(this));
    }

    @Override protected AndroidAudioRenderer createAudioRenderer() {
        audioRenderer = super.createAudioRenderer();
        audioRenderer.setMuted(FlashLinkAudio.HOST.equals(FlashLinkAudio.route(this)));
        return audioRenderer;
    }

    void audioChanged() {
        if (audioRenderer != null) audioRenderer.setMuted(FlashLinkAudio.HOST.equals(FlashLinkAudio.route(this)));
        if (privacyLease != null) privacyLease.refresh();
    }

    private void releaseControls() {
        cancelTouchscreenInput();
        if (tools != null) tools.release();
    }

    void leaveStream() {
        Log.i(LOG, "MANUAL_EXIT");
        closeLease(true);
        finish();
    }

    @Override public void onBackPressed() {
        if (tools != null && tools.collapse()) return;
        leaveStream();
    }
    @Override protected void onResume() {
        super.onResume();
        foreground = true;
        prepareSurface();
    }
    @Override public void onWindowFocusChanged(boolean focus) {
        super.onWindowFocusChanged(focus);
        if (!focus) releaseControls();
    }
    @Override protected void onPause() {
        foreground = false;
        if (pendingSurface != null) closeLease(false);
        releaseControls();
        super.onPause();
    }
    @Override public void onConfigurationChanged(Configuration config) {
        releaseControls();
        super.onConfigurationChanged(config);
    }
    @Override public void onPictureInPictureModeChanged(boolean active, Configuration config) {
        super.onPictureInPictureModeChanged(active, config);
        if (active) releaseControls();
        if (tools != null && Build.VERSION.SDK_INT >= 26) tools.setVisible(!active);
    }
    private void closeLease(boolean ended) {
        if (privacyLease != null) {
            privacyLease.close(ended);
            privacyLease = null;
        }
    }

    @Override protected void onStop() {
        closeLiveSettings();
        closeLease(true);
        super.onStop();
    }

    @Override protected void onDestroy() {
        closeLiveSettings();
        releaseControls();
        closeLease(true);
        timer.removeCallbacksAndMessages(null);
        super.onDestroy();
    }
}
