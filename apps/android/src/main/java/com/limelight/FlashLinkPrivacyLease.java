// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.app.Activity;
import android.content.Context;
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

/** A short-lived heartbeat for the currently displayed stream. Both peers prove their paired identity. */
final class FlashLinkPrivacyLease {
    private volatile boolean closed;
    private volatile Socket current;
    private final Thread worker;

    static FlashLinkPrivacyLease start(Activity activity, String host, int base, byte[] serverCert) {
        return new FlashLinkPrivacyLease(activity.getApplicationContext(), host, base, serverCert);
    }
    private FlashLinkPrivacyLease(Context context, String host, int port, byte[] serverCert) {
        worker = new Thread(() -> run(context, host, port, serverCert), "FlashLink privacy");
        worker.setDaemon(true);
        worker.start();
    }
    void close() {
        closed = true;
        Socket socket = current;
        if (socket != null) try { socket.close(); } catch (IOException ignored) { }
        worker.interrupt();
    }
    private void run(Context app, String host, int port, byte[] serverCert) {
        if (serverCert == null || host == null || port < 1032) return;
        try {
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
            while (!closed) {
                try (Socket raw = new Socket()) {
                    current = raw;
                    if (closed) return;
                    raw.connect(new InetSocketAddress(host, port - 8), 1800);
                    raw.setSoTimeout(1800);
                    try (SSLSocket stream = (SSLSocket) tls.getSocketFactory().createSocket(raw, host, port - 8, true)) {
                        stream.startHandshake();
                        stream.getOutputStream().write("FLASHLINK_STREAM/1\n".getBytes(StandardCharsets.US_ASCII));
                        stream.getOutputStream().flush();
                        stream.getInputStream().read();
                    }
                } catch (IOException ignored) { /* Media playback is independent of privacy. */ }
                finally { current = null; }
                Thread.sleep(1800);
            }
        } catch (Exception e) {
            if (!closed) android.util.Log.w("FlashLink", "隐私心跳不可用：" + e.getClass().getSimpleName());
        }
    }
}
