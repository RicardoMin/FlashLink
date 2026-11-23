// SPDX-License-Identifier: MIT
package com.limelight;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Typeface;
import android.opengl.GLSurfaceView;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.view.Gravity;
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
    private Button closeButton;
    private GLSurfaceView rendererProbe;
    private final Handler handler = new Handler();
    private boolean resumed;
    private Intent ready;
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
        setContentView(panel);
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
            runOnUiThread(() -> { ready = game; openIfResumed(); });
        } catch (Exception e) {
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
        ready.addFlags(Intent.FLAG_ACTIVITY_FORWARD_RESULT);
        startActivity(ready);
        ready = null;
        finish();
    }

    private void stopWork() {
        stopped = true;
        handler.removeCallbacks(timeout);
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
