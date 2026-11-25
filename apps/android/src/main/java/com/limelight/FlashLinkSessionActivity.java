// SPDX-License-Identifier: MIT
package com.limelight;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Typeface;
import android.opengl.GLSurfaceView;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.SystemClock;
import android.util.Log;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

import com.limelight.binding.PlatformBinding;
import com.limelight.computers.ComputerDatabaseManager;
import com.limelight.computers.IdentityManager;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.HostHttpResponseException;
import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.http.NvHTTP;
import com.limelight.nvstream.http.PairingManager;
import com.limelight.preferences.GlPreferences;
import com.limelight.utils.ServerHelper;

import java.io.IOException;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;

public final class FlashLinkSessionActivity extends Activity {
    private TextView status, pinText;
    private Button closeButton, retryButton;
    private GLSurfaceView rendererProbe;
    private final Handler handler = new Handler();
    private boolean resumed;
    private static final int PLAY_DESKTOP = 71;
    private static final String RESTORE_ATTEMPTS = "dev.flashlink.restoreAttempts";
    private static final String RESTORE_NEXT = "dev.flashlink.restoreNext";
    private final FlashLinkReconnect recovery = new FlashLinkReconnect();
    private final Runnable recoveryTimeout = this::expireRecovery;
    private final Runnable openReady = this::openIfResumed;
    private boolean gameOpen;
    private Intent ready;
    private Intent retryQuality;
    private volatile boolean stopped;
    private volatile NvHTTP current;
    private volatile FlashLinkConsent consent;
    private final Runnable timeout = () -> fail("配对已超时，请在两端重新发起连接。");

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        LinearLayout panel = new LinearLayout(this);
        panel.setGravity(Gravity.CENTER);
        panel.setPadding(32, 24, 32, 24);
        panel.setOrientation(LinearLayout.VERTICAL);
        cacheGlRenderer(panel);
        TextView title = label(panel, "FlashLink · " + getIntent().getStringExtra(FlashLinkActivity.NAME), 24);
        title.setTypeface(Typeface.DEFAULT_BOLD);
        status = label(panel, "正在连接电脑…", 17);
        pinText = label(panel, "", 25);
        pinText.setTypeface(Typeface.MONOSPACE, Typeface.BOLD);
        closeButton = new Button(this);
        closeButton.setText("取消连接");
        closeButton.setOnClickListener(v -> finish());
        panel.addView(closeButton);
        retryButton = new Button(this);
        retryButton.setText("重试原画质");
        retryButton.setVisibility(View.GONE);
        retryButton.setOnClickListener(v -> restoreQuality(retryQuality, true));
        panel.addView(retryButton);
        setContentView(panel);
        gameOpen = state != null && state.getBoolean("player");
        if (gameOpen) return; // Android will deliver the result of the existing child.
        if (state != null && state.containsKey("qualityReady")) {
            queueQuality(state.getParcelable("qualityReady"));
            return;
        }
        if (state != null && state.containsKey("qualityRetry")) {
            showQualityRetry(state.getParcelable("qualityRetry"), state.getString("qualityError"));
            return;
        }
        if (state != null && state.getBoolean("recovering")) {
            fail("连接已中断，点返回后可重新连接。");
            return;
        }
        new Thread(this::connect, "FlashLink Desktop").start();
    }

    private void cacheGlRenderer(LinearLayout panel) {
        // The upstream PC picker normally populates this cache for MediaCodec's driver workarounds.
        GlPreferences prefs = GlPreferences.readPreferences(this);
        if (Build.FINGERPRINT.equals(prefs.savedFingerprint) && !prefs.glRenderer.isEmpty()) return;
        rendererProbe = new GLSurfaceView(this);
        rendererProbe.setRenderer(new GLSurfaceView.Renderer() {
            public void onSurfaceCreated(javax.microedition.khronos.opengles.GL10 gl, javax.microedition.khronos.egl.EGLConfig config) {
                prefs.glRenderer = gl.glGetString(javax.microedition.khronos.opengles.GL10.GL_RENDERER);
                if (prefs.glRenderer != null && !prefs.glRenderer.isEmpty()) {
                    prefs.savedFingerprint = Build.FINGERPRINT;
                    prefs.writePreferences();
                }
            }
            public void onSurfaceChanged(javax.microedition.khronos.opengles.GL10 gl, int width, int height) { }
            public void onDrawFrame(javax.microedition.khronos.opengles.GL10 gl) { }
        });
        rendererProbe.setRenderMode(GLSurfaceView.RENDERMODE_WHEN_DIRTY);
        panel.addView(rendererProbe, 1, 1);
    }

    private TextView label(LinearLayout panel, String text, int size) {
        TextView view = new TextView(this);
        view.setGravity(Gravity.CENTER);
        view.setText(text);
        view.setTextSize(size);
        view.setPadding(0, 10, 0, 10);
        panel.addView(view);
        return view;
    }

    private NvHTTP client(ComputerDetails.AddressTuple address, int httpsPort, String uid, X509Certificate cert) throws IOException {
        NvHTTP http = new NvHTTP(address, httpsPort, uid, cert, PlatformBinding.getCryptoProvider(this));
        current = http;
        if (stopped) throw new IOException("Session closed");
        return http;
    }

    private void connect() {
        ComputerDatabaseManager database = null;
        String stage = "打开本机证书库";
        try {
            String digest = getIntent().getStringExtra(FlashLinkActivity.ID);
            String host = getIntent().getStringExtra(FlashLinkActivity.HOST);
            int port = getIntent().getIntExtra(FlashLinkActivity.PORT, NvHTTP.DEFAULT_HTTP_PORT);
            if (recovery.attempts > 1) {
                try {
                    for (java.util.Map<String, String> nearby : FlashLinkDiscovery.scan(this)) {
                        if (digest.equals(nearby.get("id"))) {
                            java.net.URI updated = FlashLinkActivity.parseAddress(nearby.get("host"));
                            host = updated.getHost();
                            port = updated.getPort();
                            break;
                        }
                    }
                } catch (IOException ignored) {
                    // A failed broadcast should not prevent a direct retry of the paired host.
                }
            }
            ComputerDetails.AddressTuple address = new ComputerDetails.AddressTuple(host, port);
            String uid = new IdentityManager(this).getUniqueId();
            database = new ComputerDatabaseManager(this);
            X509Certificate trusted = null;
            for (ComputerDetails known : database.getAllComputers()) {
                if (known.serverCert != null && digest.equals(FlashLinkConsent.fingerprint(known.serverCert))) {
                    trusted = known.serverCert;
                    break;
                }
            }
            NvHTTP http = client(address, port - 5, uid, trusted);
            ComputerDetails details = null;
            if (trusted != null) {
                try {
                    stage = "验证已配对的电脑";
                    details = http.getComputerDetails(http.getAuthenticatedServerInfo());
                } catch (HostHttpResponseException e) {
                    if (e.getErrorCode() != 401 && e.getErrorCode() != 403) throw e;
                }
            }
            if (details == null || details.pairState != PairingManager.PairState.PAIRED) {
                if (recovery.deadline > 0) {
                    throw new UserVisibleError("此电脑已取消配对。请从设备列表重新连接并授权。");
                }
                consent = new FlashLinkConsent(digest);
                if (stopped) return;
                handler.postDelayed(timeout, 90000);
                stage = "发送加密授权请求";
                X509Certificate leaf = consent.queue(host, port);
                http = client(address, port - 5, uid, null);
                stage = "读取电脑串流信息";
                String metadata = http.getServerInfo(false);
                http.setFlashLinkPairing(consent.token, leaf);
                runOnUiThread(() -> {
                    if (!stopped) {
                        status.setText("请核对电脑上的验证码，并在电脑上点击允许。\n本机设备：" + FlashLinkConsent.deviceName());
                        pinText.setText(consent.verificationCode());
                    }
                });
                PairingManager manager = http.getPairingManager();
                stage = "完成配对挑战";
                PairingManager.PairState paired = manager.pair(metadata, consent.pin);
                if (stopped) return;
                if (paired != PairingManager.PairState.PAIRED) {
                    throw new UserVisibleError("电脑未授权此连接，或验证码已失效。请重新连接。");
                }
                trusted = manager.getPairedCert();
                if (!leaf.equals(trusted)) throw new CertificateException("Pairing certificate mismatch");
                http = client(address, port - 5, uid, trusted);
                stage = "验证配对结果";
                details = http.getComputerDetails(http.getAuthenticatedServerInfo());
                if (details.pairState != PairingManager.PairState.PAIRED) {
                    throw new UserVisibleError("电脑没有确认配对，请重新连接。");
                }
                handler.removeCallbacks(timeout);
            }
            details.serverCert = trusted;
            details.activeAddress = address;
            details.manualAddress = address;
            if (stopped) return;
            if (!database.updateComputer(details)) throw new UserVisibleError("无法保存配对凭证，请检查设备存储。");
            runOnUiThread(() -> {
                if (!stopped) { pinText.setText(""); status.setText("正在打开 Windows 桌面…"); }
            });
            stage = "读取桌面列表";
            NvApp desktop = http.getAppByName("Desktop");
            if (desktop == null) throw new UserVisibleError("电脑未提供 Desktop 桌面，请检查 Windows 被控端。");
            if (details.runningGameId != 0 && details.runningGameId != desktop.getAppId()) {
                throw new UserVisibleError("电脑正在串流另一个应用，请先结束那次串流。");
            }
            Intent game = ServerHelper.createStartIntent(this, desktop, details, uid);
            game.setClass(this, FlashLinkGame.class);
            game.putExtra(FlashLinkGame.DEADLINE, recovery.deadline);
            game.putExtra(FlashLinkAudio.MANAGED, FlashLinkPrivacyLease.supportsAudio(
                    getApplicationContext(), host, port, trusted.getEncoded()));
            runOnUiThread(() -> { if (!stopped) { ready = game; openIfResumed(); } });
        } catch (Exception e) {
            if (stopped) return;
            if (recovery.deadline > 0 && e instanceof IOException
                    && !(e instanceof javax.net.ssl.SSLException) && !(e instanceof HostHttpResponseException)) {
                runOnUiThread(this::scheduleRecovery);
                return;
            }
            android.util.Log.w("FlashLink", stage + " · " + e.getClass().getSimpleName());
            String message = e instanceof UserVisibleError ? e.getMessage()
                    : e instanceof javax.net.ssl.SSLException || e instanceof CertificateException
                    ? "无法核实电脑身份，请重新搜索并核对电脑。"
                    : stage + "失败。请确认电脑仍在运行 FlashLink，并检查电脑上的授权提示。";
            runOnUiThread(() -> fail(message));
        } finally {
            if (database != null) database.close();
            handler.removeCallbacks(timeout);
        }
    }

    private void openIfResumed() {
        if (ready == null || !resumed || stopped || isFinishing()) return;
        long now = SystemClock.elapsedRealtime();
        if (recovery.deadline > 0 && now >= recovery.deadline) {
            expireRecovery();
            return;
        }
        if (ready.getBooleanExtra(FlashLinkGame.RESTORING, false)) {
            if (now >= ready.getLongExtra(FlashLinkGame.RESTORE_UNTIL, 0)) {
                String lastError = ready.getStringExtra(FlashLinkGame.RESTORE_FAILURE);
                showQualityRetry(ready, "自动恢复原画质已超时。" + (lastError == null ? "" : "\n" + lastError)
                        + "\n请确认电脑和网络状态后重试。");
                return;
            }
            long wait = ready.getLongExtra(RESTORE_NEXT, 0) - now;
            if (wait > 0) {
                handler.removeCallbacks(openReady);
                handler.postDelayed(openReady, wait);
                return;
            }
        }
        handler.removeCallbacks(recoveryTimeout);
        gameOpen = true;
        startActivityForResult(ready, PLAY_DESKTOP);
        ready = null;
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != PLAY_DESKTOP) return;
        gameOpen = false;
        if ((result == FlashLinkGame.QUALITY_CHANGE || result == FlashLinkGame.QUALITY_FAILED) && data != null) {
            recovery.deadline = 0;
            recovery.attempts = 0;
            handler.removeCallbacks(recoveryTimeout);
            if (result == FlashLinkGame.QUALITY_CHANGE) {
                queueQuality(data);
            } else if (data.getBooleanExtra(FlashLinkGame.RESTORING, false)) {
                if (data.getBooleanExtra(FlashLinkGame.RETRYABLE, false)) {
                    restoreQuality(data, false);
                } else {
                    showQualityRetry(data, "恢复原画质失败：" + data.getStringExtra(FlashLinkGame.RESTORE_FAILURE)
                            + "\n请确认电脑和网络状态后重试。");
                }
            } else {
                data.putExtra(FlashLinkQuality.EXTRA, data.getIntArrayExtra(FlashLinkGame.PREVIOUS_QUALITY));
                data.putExtra(FlashLinkGame.RESTORING, true);
                restoreQuality(data, true);
            }
        } else if (result == FlashLinkGame.EXPIRED) {
            expireRecovery();
        } else if (result == FlashLinkGame.NETWORK_LOST && data != null) {
            long deadline = data.getLongExtra(FlashLinkGame.DEADLINE, 0);
            if (recovery.deadline != deadline) recovery.interrupted(SystemClock.elapsedRealtime(), deadline);
            scheduleRecovery();
        } else {
            finish();
        }
    }

    private void restoreQuality(Intent game, boolean first) {
        long now = SystemClock.elapsedRealtime();
        FlashLinkReconnect retry = new FlashLinkReconnect();
        // Allow Sunshine's pending RTSP launch to expire, then leave time to negotiate and decode.
        long deadline = first ? now + 30_000 : game.getLongExtra(FlashLinkGame.RESTORE_UNTIL, now);
        retry.interrupted(now, deadline);
        retry.attempts = first ? 0 : game.getIntExtra(RESTORE_ATTEMPTS, 0);
        long delay = first ? 0 : retry.nextDelay(now);
        game.putExtra(FlashLinkGame.RESTORE_UNTIL, retry.deadline);
        game.putExtra(RESTORE_ATTEMPTS, first ? 0 : retry.attempts + 1);
        game.putExtra(RESTORE_NEXT, delay < 0 ? retry.deadline : now + delay);
        queueQuality(game);
    }

    private void queueQuality(Intent game) {
        FlashLinkQuality selected = FlashLinkQuality.fromArray(game.getIntArrayExtra(FlashLinkQuality.EXTRA));
        if (selected == null) {
            fail("无法读取串流参数，请重新连接。");
            return;
        }
        // This private child result carries the same pinned host, client identity and Desktop app ID.
        game.setClass(this, FlashLinkGame.class);
        ready = game;
        retryQuality = null;
        retryButton.setVisibility(View.GONE);
        boolean restoring = game.getBooleanExtra(FlashLinkGame.RESTORING, false);
        String restoreStatus = game.getIntExtra(RESTORE_ATTEMPTS, 0) > 0 ? "正在重试恢复上次画质…" : "正在恢复上次画质…";
        status.setText(restoring ? "画质切换失败：" + game.getStringExtra(FlashLinkGame.FAILURE)
                + "\n" + restoreStatus : "正在为同一台电脑重新协商画质…");
        pinText.setText(selected.displayText());
        closeButton.setText("取消连接");
        openIfResumed();
    }

    private void showQualityRetry(Intent game, String message) {
        ready = null;
        retryQuality = game;
        pinText.setText("");
        status.setText(message);
        retryButton.setVisibility(View.VISIBLE);
        closeButton.setText("返回设备列表");
    }

    private void scheduleRecovery() {
        if (stopped || isFinishing()) return;
        long now = SystemClock.elapsedRealtime();
        long delay = recovery.nextDelay(now);
        if (now >= recovery.deadline) {
            expireRecovery();
            return;
        }
        pinText.setText("");
        status.setText("连接中断，正在尝试重连…");
        closeButton.setText("取消重连");
        handler.removeCallbacks(recoveryTimeout);
        handler.postDelayed(recoveryTimeout, recovery.deadline - now);
        if (delay < 0) return;
        handler.postDelayed(() -> {
            if (stopped || !resumed) return;
            recovery.attempts++;
            Log.i(FlashLinkGame.LOG, "RETRY_ATTEMPT");
            new Thread(this::connect, "FlashLink reconnect").start();
        }, delay);
    }

    private void expireRecovery() {
        if (stopped || isFinishing()) return;
        Log.i(FlashLinkGame.LOG, "RETRY_EXHAUSTED");
        fail("自动重连已超时，请确认电脑和 Wi-Fi 状态后重新连接。");
    }

    @Override protected void onSaveInstanceState(Bundle state) {
        state.putBoolean("player", gameOpen);
        state.putBoolean("recovering", recovery.deadline > 0);
        if (ready != null && ready.hasExtra(FlashLinkQuality.EXTRA)) state.putParcelable("qualityReady", ready);
        if (retryQuality != null) {
            state.putParcelable("qualityRetry", retryQuality);
            state.putString("qualityError", status.getText().toString());
        }
        super.onSaveInstanceState(state);
    }

    @Override protected void onStop() {
        if (!gameOpen && !isFinishing() && (recovery.deadline > 0 ||
                (ready != null && ready.hasExtra(FlashLinkQuality.EXTRA)))) finish();
        super.onStop();
    }

    private void stopWork() {
        stopped = true;
        handler.removeCallbacksAndMessages(null);
        FlashLinkConsent pending = consent;
        if (pending != null) pending.close();
        NvHTTP http = current;
        if (http != null) http.cancelFlashLinkRequests();
    }

    private void fail(String message) {
        if (stopped || isFinishing()) return;
        stopWork();
        pinText.setText("");
        status.setText(message);
        closeButton.setText("返回设备列表");
    }

    @Override protected void onResume() {
        super.onResume();
        if (rendererProbe != null) rendererProbe.onResume();
        resumed = true;
        openIfResumed();
    }
    @Override protected void onPause() {
        resumed = false;
        if (rendererProbe != null) rendererProbe.onPause();
        super.onPause();
    }
    @Override public void finish() { stopWork(); super.finish(); }
    @Override protected void onDestroy() { stopWork(); super.onDestroy(); }

    private static final class UserVisibleError extends Exception {
        UserVisibleError(String message) { super(message); }
    }
}
