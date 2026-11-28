// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.app.Activity;
import android.content.Context;
import android.content.SharedPreferences;
import android.media.AudioManager;
import android.os.Build;
import android.preference.PreferenceManager;
import com.limelight.binding.PlatformBinding;
import com.limelight.nvstream.http.LimelightCryptoProvider;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.Principal;
import java.security.PrivateKey;
import java.security.MessageDigest;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import javax.net.ssl.*;

/** Controller-only playback can wait for an initial mute confirmation. Heartbeats run off the UI thread. */
final class FlashLinkPrivacyLease {
    private static final int INTERVAL_MS = 1800;
    private final Context app;
    private final boolean managed;
    private final FlashLinkAudioControl.Session session;
    interface Preflight {
        void result(FlashLinkPrivacyLease lease, boolean muted);
    }

    private final Preflight preflight;
    private String finalSample;
    private String lastVolume;
    private int revision;

    static FlashLinkPrivacyLease start(Activity activity, String host, int base, byte[] serverCert,
                                       boolean managed, Preflight preflight) {
        return new FlashLinkPrivacyLease(activity.getApplicationContext(), host, base, serverCert, managed, preflight);
    }

    // The host can discard a late final report from an Activity replaced by a newer stream.
    private static synchronized long nextSession(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        long next = Math.max(System.currentTimeMillis(), prefs.getLong("flashlink_audio_session", 0) + 1);
        prefs.edit().putLong("flashlink_audio_session", next).apply();
        return next;
    }

    private FlashLinkPrivacyLease(Context context, String host, int base, byte[] serverCert,
                                  boolean managed, Preflight preflight) {
        this.preflight = preflight;
        this.app = context;
        this.managed = managed;
        this.session = new FlashLinkAudioControl.Session(managed ? nextSession(context) : 0, preflight != null);
        Thread worker = new Thread(() -> run(host, base, serverCert), "FlashLink stream state");
        worker.setDaemon(true);
        worker.start();
    }

    synchronized boolean canStart() {
        return session.canStart(android.os.SystemClock.elapsedRealtime());
    }

    synchronized void connected() {
        session.connected();
        refresh();
    }

    synchronized void refresh() {
        revision++;
        notifyAll();
    }

    void close(boolean ended) {
        finish(ended ? "end" : "drop");
    }

    void handoff() {
        finish("handoff");
    }

    private synchronized void finish(String phase) {
        if (!session.finish(phase)) return;
        if (managed && !session.phase().equals("drop")) finalSample = sample();
        notifyAll();
    }

    // Called from the session's connection worker after the host and client are paired.
    static int audioCapability(Context context, String host, int base, byte[] serverCert) throws Exception {
        SSLContext tls = authenticatedTls(context, serverCert);
        return FlashLinkAudioControl.capability(message -> exchange(tls, host, base, message));
    }

    private static SSLContext authenticatedTls(Context app, byte[] serverCert) throws Exception {
        LimelightCryptoProvider identity = PlatformBinding.getCryptoProvider(app);
        X509KeyManager key = new X509KeyManager() {
            public String chooseClientAlias(String[] types, Principal[] issuers, Socket socket) { return "paired"; }
            public String chooseServerAlias(String type, Principal[] issuers, Socket socket) { return null; }
            public String[] getClientAliases(String type, Principal[] issuers) { return new String[]{"paired"}; }
            public String[] getServerAliases(String type, Principal[] issuers) { return null; }
            public PrivateKey getPrivateKey(String name) { return identity.getClientPrivateKey(); }
            public X509Certificate[] getCertificateChain(String name) { return new X509Certificate[]{identity.getClientCertificate()}; }
        };
        X509TrustManager pinned = new X509TrustManager() {
            public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
            public void checkClientTrusted(X509Certificate[] chain, String type) throws CertificateException {
                throw new CertificateException("Expected host");
            }
            public void checkServerTrusted(X509Certificate[] chain, String type) throws CertificateException {
                if (chain.length == 0 || !MessageDigest.isEqual(serverCert, chain[0].getEncoded()))
                    throw new CertificateException("Host identity changed");
            }
        };
        SSLContext tls = SSLContext.getInstance("TLS");
        tls.init(new KeyManager[]{key}, new TrustManager[]{pinned}, null);
        return tls;
    }

    private static boolean exchange(SSLContext tls, String host, int base, String message) throws IOException {
        try (Socket raw = new Socket()) {
            raw.connect(new InetSocketAddress(host, base - 8), INTERVAL_MS);
            raw.setSoTimeout(INTERVAL_MS);
            try (SSLSocket stream = (SSLSocket) tls.getSocketFactory().createSocket(raw, host, base - 8, true)) {
                stream.startHandshake();
                stream.getOutputStream().write((message + "\n").getBytes(StandardCharsets.US_ASCII));
                stream.getOutputStream().flush();
                return FlashLinkAudioControl.readReply(stream.getInputStream());
            }
        }
    }

    private String sample() {
        AudioManager audio = (AudioManager) app.getSystemService(Context.AUDIO_SERVICE);
        int music = AudioManager.STREAM_MUSIC;
        if (audio != null) {
            int max = audio.getStreamMaxVolume(music);
            if (max > 0) {
                int min = Build.VERSION.SDK_INT >= 28 ? audio.getStreamMinVolume(music) : 0;
                int level = audio.getStreamVolume(music);
                int percent = max > min ? Math.round(100f * (level - min) / (max - min)) : 100;
                lastVolume = Math.max(0, Math.min(100, percent)) + " " + (audio.isStreamMute(music) ? "1" : "0");
            }
        }
        String route = (session.preparing() || session.phase().equals("cancel")) ? FlashLinkAudio.CONTROLLER : FlashLinkAudio.route(app);
        return lastVolume == null ? null : route + " " + FlashLinkAudio.after(app) + " " + lastVolume;
    }

    private void run(String host, int base, byte[] serverCert) {
        boolean reportedFailure = false;
        try {
            SSLContext tls = authenticatedTls(app, serverCert);
            FlashLinkAudioControl.Request request = message -> exchange(tls, host, base, message);
            while (true) {
                final String phase;
                final String message;
                final boolean terminal;
                final boolean hasAudio;
                final int version;
                synchronized (this) {
                    terminal = session.terminal();
                    phase = session.phase();
                    if ("drop".equals(phase) || (!managed && terminal)) return;
                    String audio = managed ? (terminal ? finalSample : sample()) : null;
                    hasAudio = audio != null;
                    message = session.report(audio);
                    version = revision;
                }
                boolean accepted = false;
                try {
                    if (message != null) {
                        int attempts = terminal || phase.equals("prepare") ? 2 : 1;
                        accepted = FlashLinkAudioControl.exchange(request, message, attempts);
                    }
                } catch (IOException ignored) {
                    // The next beat retries live state. Final messages retry the same sequence.
                }
                if (phase.equals("prepare")) {
                    // A duplicate prepare can be in flight while the UI promotes this lease to live.
                    synchronized (this) {
                        if (session.preparing()) {
                            session.confirmed(accepted, android.os.SystemClock.elapsedRealtime());
                            preflight.result(this, accepted);
                            if (!accepted) finish("cancel");
                        }
                    }
                } else if (managed && (!accepted || !hasAudio) && !reportedFailure && !phase.equals("cancel")) {
                    warnUnconfirmed();
                    reportedFailure = true;
                } else if (accepted && hasAudio) {
                    reportedFailure = false;
                }
                if (terminal) return;
                synchronized (this) {
                    if (!session.terminal() && revision == version) wait(INTERVAL_MS);
                }
            }
        } catch (Exception e) {
            android.util.Log.w("FlashLink", "串流状态同步不可用：" + e.getClass().getSimpleName());
            synchronized (this) {
                if (session.preparing()) preflight.result(this, false);
                else if (!session.terminal() && managed) warnUnconfirmed();
            }
        }
    }

    private void warnUnconfirmed() {
        new android.os.Handler(android.os.Looper.getMainLooper()).post(() ->
                android.widget.Toast.makeText(app, "电脑未确认本次声音设置，请检查主机和局域网连接。",
                        android.widget.Toast.LENGTH_LONG).show());
    }
}
