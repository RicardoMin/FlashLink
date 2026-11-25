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
import java.io.InputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.Principal;
import java.security.PrivateKey;
import java.security.MessageDigest;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import javax.net.ssl.*;

/** Authenticated state from the displayed stream. Media playback never waits on the control socket. */
final class FlashLinkPrivacyLease {
    private static final int INTERVAL_MS = 1800;
    private final Context app;
    private final boolean managed;
    private final long session;
    private String closing;
    private String finalSample;
    private String lastVolume;
    private int revision;

    static FlashLinkPrivacyLease start(Activity activity, String host, int base, byte[] serverCert, boolean managed) {
        return new FlashLinkPrivacyLease(activity.getApplicationContext(), host, base, serverCert, managed);
    }

    // The host can discard a late final report from an Activity replaced by a newer stream.
    private static synchronized long nextSession(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        long next = Math.max(System.currentTimeMillis(), prefs.getLong("flashlink_audio_session", 0) + 1);
        prefs.edit().putLong("flashlink_audio_session", next).apply();
        return next;
    }

    private FlashLinkPrivacyLease(Context context, String host, int base, byte[] serverCert, boolean managed) {
        this.app = context;
        this.managed = managed;
        this.session = managed ? nextSession(context) : 0;
        Thread worker = new Thread(() -> run(host, base, serverCert), "FlashLink stream state");
        worker.setDaemon(true);
        worker.start();
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
        if (closing != null) return;
        closing = phase;
        if (managed && !phase.equals("drop")) finalSample = sample();
        notifyAll();
    }

    // Called from the session's connection worker after the host and client are paired.
    static boolean supportsAudio(Context context, String host, int base, byte[] serverCert) {
        if (host == null || serverCert == null || base < 1032) return false;
        try {
            return exchange(authenticatedTls(context, serverCert), host, base, "FLASHLINK_AUDIO_CAPS/1");
        } catch (Exception e) {
            return false;
        }
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
                InputStream reply = stream.getInputStream();
                return reply.read() == 'O' && reply.read() == 'K' && reply.read() == '\n';
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
        return lastVolume == null ? null : FlashLinkAudio.route(app) + " " + FlashLinkAudio.after(app) + " " + lastVolume;
    }

    private void run(String host, int base, byte[] serverCert) {
        if (serverCert == null || host == null || base < 1032) return;
        try {
            SSLContext tls = authenticatedTls(app, serverCert);
            long sequence = 0;
            while (true) {
                final String phase;
                final String audio;
                final int version;
                synchronized (this) {
                    phase = closing;
                    if ("drop".equals(phase) || (!managed && phase != null)) return;
                    audio = managed ? (phase == null ? sample() : finalSample) : null;
                    version = revision;
                }
                String message = "FLASHLINK_STREAM/1";
                if (audio != null) {
                    message = "FLASHLINK_STREAM/2 " + session + " " + (++sequence) + " "
                            + (phase == null ? "live" : phase) + " " + audio;
                }
                // A manual exit may coincide with a transient socket failure. The same
                // sequence is safe to retry when the acknowledgement was lost.
                int attempts = phase == null ? 1 : 2;
                for (int i = 0; i < attempts; i++) {
                    try {
                        exchange(tls, host, base, message);
                        break;
                    } catch (IOException ignored) {
                        // On network loss the host still has the last delivered sample.
                    }
                }
                if (phase != null) return;
                synchronized (this) {
                    if (closing == null && revision == version) wait(INTERVAL_MS);
                }
            }
        } catch (Exception e) {
            android.util.Log.w("FlashLink", "串流状态同步不可用：" + e.getClass().getSimpleName());
        }
    }
}
