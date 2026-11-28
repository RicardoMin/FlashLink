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
import android.view.MotionEvent;
import android.view.SurfaceHolder;
import android.view.View;
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
    static final String LOG = "FlashLinkSession";
    private static final String SHOW_STATUS = "flashlink_stream_status";
    private final Handler timer = new Handler();
    private final Runnable expire = () -> endWith(EXPIRED, 0);
    private long recoveryDeadline;
    private long startedAt;
    private boolean foreground;
    private FlashLinkStreamTools tools;
    private volatile FlashLinkStreamStatus streamStatus;
    private boolean poorConnection;
    private FlashLinkStreamTouch streamTouch;
    private FlashLinkPrivacyLease privacyLease;
    private AndroidAudioRenderer audioRenderer;
    private boolean mediaRequested;
    private SurfaceHolder pendingSurface;
    private Runnable startMedia;
    private FlashLinkQuality activeQuality;
    private FlashLinkQuality requestedQuality;

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
                continueQualityChange();
            } else {
                failQualityChange("电脑未能切换至所选分辨率。");
            }
        }

        @Override public void unconfirmed() {
            setDecoderResizing(false);
            failQualityChange("尚未确认电脑画面尺寸。");
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
                    if (foreground && !isFinishing() && !isDestroyed()) openQualityDialog();
                }
            }
        });
    }

    private void onSettingResult(boolean fps, FlashLinkLiveSetting.Result result, int value) {
        String label = fps ? "帧率" : "画质";
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
                continueQualityChange();
                break;
            case REJECTED:
                failQualityChange("电脑未应用所选" + label + "。");
                break;
            case UNCONFIRMED:
                failQualityChange("尚未确认电脑当前" + label + "。");
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
        new FlashLinkQualityDialog(this, activeQuality).show();
    }

    private void closeLiveSettings() {
        openQualityWhenReady = false;
        liveBitrate.close();
        liveFps.close();
        liveSize.close();
        resolution.close();
        requestedQuality = null;
        settingSender.shutdownNow();
    }

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        recoveryDeadline = getIntent().getLongExtra(DEADLINE, 0);
        if (recoveryDeadline > 0) {
            timer.postDelayed(expire, Math.max(0, recoveryDeadline - SystemClock.elapsedRealtime()));
        }
    }

    @Override public void connectionStarted() {
        super.connectionStarted();
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed()) return;
            startedAt = SystemClock.elapsedRealtime();
            if (recoveryDeadline > 0 && startedAt >= recoveryDeadline) {
                expire.run();
                return;
            }
            timer.removeCallbacks(expire);
            Log.i(LOG, recoveryDeadline == 0 ? "STREAM_STARTED" : "STREAM_RESUMED");
            if (privacyLease == null) privacyLease = startLease(null);
            else privacyLease.connected();
            if (!managedAudio() && recoveryDeadline == 0) {
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
        if (activeQuality == null || isFinishing()) return;
        if (activeQuality.width != width || activeQuality.height != height) {
            updateStreamSize(width, height);
            activeQuality = new FlashLinkQuality(width, height, activeQuality.fps, activeQuality.bitrateKbps);
        }
        Log.i(LOG, "RESOLUTION_PRESENTED " + width + "x" + height);
        resolution.presented(FlashLinkResolution.pack(width, height));
    }

    @Override protected void recoverOversizedStream(int size) {
        if (resolution.inputTooSmall(size)) {
            requestedQuality = null;
            notifySetting("新尺寸的画面超出解码缓冲，正在尝试恢复上次分辨率。");
        }
    }

    private void streamReady() {
        if (startedAt == 0 || activeQuality == null || tools != null || isFinishing() || isDestroyed()) return;
        streamStatus = new FlashLinkStreamStatus(this);
        streamStatus.setPoorConnection(poorConnection);
        refreshStatusOverlay();
        tools = new FlashLinkStreamTools(this);
        tools.attach();
        tools.setVisible(!isInPictureInPictureMode());
        liveBitrate.probe();
        liveFps.probe();
        liveSize.probe();
    }

    FlashLinkQuality currentQuality() { return activeQuality; }

    boolean isStatusOverlayEnabled() {
        return PreferenceManager.getDefaultSharedPreferences(this).getBoolean(SHOW_STATUS, false);
    }

    void setStatusOverlayEnabled(boolean enabled) {
        PreferenceManager.getDefaultSharedPreferences(this).edit().putBoolean(SHOW_STATUS, enabled).apply();
        refreshStatusOverlay();
    }

    private void refreshStatusOverlay() {
        if (streamStatus != null) {
            streamStatus.setShown(foreground && !isInPictureInPictureMode() && isStatusOverlayEnabled());
        }
    }

    @Override public void onRenderedFrameRate(int fps) {
        FlashLinkStreamStatus status = streamStatus;
        if (status != null) status.onSample(fps);
    }

    @Override public void connectionStatusUpdate(int connectionStatus) {
        super.connectionStatusUpdate(connectionStatus);
        runOnUiThread(() -> {
            poorConnection = connectionStatus == MoonBridge.CONN_STATUS_POOR;
            if (streamStatus != null) streamStatus.setPoorConnection(poorConnection);
        });
    }

    String liveQualityRestriction(FlashLinkQuality selected) {
        boolean resize = selected.width != activeQuality.width || selected.height != activeQuality.height;
        if ((resize && liveSize.state() != FlashLinkLiveSetting.State.READY) ||
                (selected.fps != activeQuality.fps && liveFps.state() != FlashLinkLiveSetting.State.READY) ||
                (selected.bitrateKbps != activeQuality.bitrateKbps && liveBitrate.state() != FlashLinkLiveSetting.State.READY)) {
            return "当前电脑未提供所选参数的实时切换，请确认 Windows 主机已更新。";
        }
        if (resize && (!FlashLinkResolution.valid(FlashLinkResolution.pack(selected.width, selected.height)) ||
                !supportsLiveSize(selected.width, selected.height, selected.fps))) {
            return "当前解码器无法在播放中切换至所选分辨率和帧率。";
        }
        if (!FlashLinkLiveSetting.Kind.FRAME_RATE.valid(selected.fps) && selected.fps != activeQuality.fps) {
            return "当前串流支持的帧率范围为 1–240帧。";
        }
        return null;
    }

    void applyQuality(FlashLinkQuality selected) {
        if (tools == null || activeQuality == null || waitingForSetting() || requestedQuality != null) return;
        String reason = liveQualityRestriction(selected);
        if (reason != null) {
            notifySetting(reason);
            return;
        }
        requestedQuality = selected;
        continueQualityChange();
    }

    private void continueQualityChange() {
        FlashLinkQuality target = requestedQuality;
        if (target == null) return;
        FlashLinkLiveSetting.Kind next = activeQuality.nextChange(target);
        if (next == null) {
            requestedQuality = null;
            notifySetting("画质已切换至 " + activeQuality.displayText());
            return;
        }
        boolean queued;
        switch (next) {
            case FRAME_RATE:
                queued = liveFps.set(target.fps);
                break;
            case RESOLUTION:
                queued = resolution.begin(FlashLinkResolution.pack(activeQuality.width, activeQuality.height),
                        FlashLinkResolution.pack(target.width, target.height));
                break;
            case BITRATE:
                queued = liveBitrate.set(target.bitrateKbps);
                break;
            default:
                throw new AssertionError(next);
        }
        if (!queued) failQualityChange("暂时无法切换串流设置。");
    }

    private void failQualityChange(String reason) {
        requestedQuality = null;
        notifySetting(reason + " 上次确认的画质为 " + activeQuality.displayText());
    }

    @Override public void connectionTerminated(int code) {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed()) return;
            if (streamStatus != null) {
                streamStatus.setShown(false);
                streamStatus = null;
            }
            releaseControls();
            closeLiveSettings();
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
        if (isFinishing()) return;
        runOnUiThread(() -> { if (startedAt == 0) closeLease(false); });
        if (recoveryDeadline > 0 && ports != 0 && (error == 0 || FlashLinkReconnect.transportLost(error))) {
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
        if (tools == null || !foreground || !hasWindowFocus() || isFinishing()) return true;
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
            if (privacyLease != lease || isFinishing() || isDestroyed()) return;
            if (!muted) {
                closeLease(false);
                String reason = "电脑未确认扬声器已静音。请检查电脑的播放设备和局域网后重新连接。";
                Toast.makeText(this, reason, Toast.LENGTH_LONG).show();
                finish();
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
        refreshStatusOverlay();
        prepareSurface();
    }
    @Override public void onWindowFocusChanged(boolean focus) {
        super.onWindowFocusChanged(focus);
        if (!focus) releaseControls();
    }
    @Override protected void onPause() {
        foreground = false;
        refreshStatusOverlay();
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
        refreshStatusOverlay();
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
