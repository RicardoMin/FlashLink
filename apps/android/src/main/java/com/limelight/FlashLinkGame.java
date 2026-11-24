// SPDX-License-Identifier: MIT
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
import android.view.View;

import com.limelight.nvstream.NvConnection;
import com.limelight.preferences.PreferenceConfiguration;

/** Thin shell around Moonlight's real Activity and its native callbacks. */
public final class FlashLinkGame extends Game {
    static final String DEADLINE = "dev.flashlink.recoveryDeadline";
    static final int NETWORK_LOST = RESULT_FIRST_USER + 1;
    static final int EXPIRED = RESULT_FIRST_USER + 2;
    static final String LOG = "FlashLinkSession";
    private final Handler timer = new Handler();
    private final Runnable expire = () -> endWith(EXPIRED, 0);
    private long recoveryDeadline;
    private long startedAt;
    private boolean foreground;
    private FlashLinkStreamTools tools;
    private FlashLinkStreamTouch streamTouch;
    private FlashLinkPrivacyLease privacyLease;

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
            privacyLease = FlashLinkPrivacyLease.start(this, getIntent().getStringExtra(EXTRA_HOST),
                    getIntent().getIntExtra(EXTRA_PORT, com.limelight.nvstream.http.NvHTTP.DEFAULT_HTTP_PORT),
                    getIntent().getByteArrayExtra(EXTRA_SERVER_CERT));
            tools = new FlashLinkStreamTools(this);
            tools.attach();
        });
    }

    @Override public void connectionTerminated(int code) {
        runOnUiThread(() -> {
            releaseControls();
            closeLease();
        });
        if (!FlashLinkReconnect.transportLost(code)) {
            timer.removeCallbacks(expire);
            super.connectionTerminated(code);
            return;
        }
        runOnUiThread(() -> {
            if (!foreground || isFinishing() || isDestroyed()) return;
            if (startedAt == 0 && recoveryDeadline == 0) {
                // Initial launch never established a stream. Keep the native error screen.
                new Thread(() -> super.connectionTerminated(code), "FlashLink stream error").start();
                return;
            }
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

    private void releaseControls() {
        cancelTouchscreenInput();
        if (tools != null) tools.release();
    }

    void leaveStream() {
        Log.i(LOG, "MANUAL_EXIT");
        finish();
    }

    @Override public void onBackPressed() {
        if (tools != null && tools.collapse()) return;
        leaveStream();
    }
    @Override protected void onResume() { super.onResume(); foreground = true; }
    @Override public void onWindowFocusChanged(boolean focus) {
        super.onWindowFocusChanged(focus);
        if (!focus) releaseControls();
    }
    @Override protected void onPause() {
        foreground = false;
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
    private void closeLease() {
        if (privacyLease != null) {
            privacyLease.close();
            privacyLease = null;
        }
    }

    @Override protected void onStop() {
        closeLease();
        super.onStop();
    }

    @Override protected void onDestroy() {
        releaseControls();
        closeLease();
        timer.removeCallbacks(expire);
        super.onDestroy();
    }
}
