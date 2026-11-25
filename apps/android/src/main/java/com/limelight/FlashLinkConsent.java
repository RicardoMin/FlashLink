// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.os.Build;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManager;
import javax.net.ssl.X509TrustManager;

/** A pending approval only. Trust is saved after Moonlight completes its independent PIN challenge. */
final class FlashLinkConsent {
    static String hex(byte[] bytes) {
        char[] alphabet = "0123456789abcdef".toCharArray();
        char[] chars = new char[bytes.length * 2];
        for (int i = 0; i < bytes.length; i++) {
            chars[i * 2] = alphabet[(bytes[i] & 255) >>> 4];
            chars[i * 2 + 1] = alphabet[bytes[i] & 15];
        }
        return new String(chars);
    }

    static String fingerprint(X509Certificate cert) throws Exception {
        return hex(MessageDigest.getInstance("SHA-256").digest(cert.getEncoded()));
    }

    static String deviceName() {
        String model = Build.MODEL.replaceAll("[\\p{C}]", " ").trim();
        return "FlashLink Android " + model.substring(0, Math.min(48, model.length()));
    }

    final String token;
    final String pin;
    final String targetId;
    private final Socket transport = new Socket();

    FlashLinkConsent(String targetId) {
        this.targetId = targetId;
        SecureRandom random = new SecureRandom();
        byte[] bytes = new byte[16];
        random.nextBytes(bytes);
        token = hex(bytes);
        pin = String.format(java.util.Locale.ROOT, "%04d", random.nextInt(10000));
    }

    String verificationCode() {
        String prefix = targetId.substring(0, 12).toUpperCase(java.util.Locale.ROOT);
        return prefix.substring(0, 4) + " " + prefix.substring(4, 8) + " "
                + prefix.substring(8, 12) + " " + pin;
    }

    void close() {
        try { transport.close(); } catch (IOException ignored) { }
    }

    X509Certificate queue(String host, int port) throws Exception {
        X509TrustManager verifier = new X509TrustManager() {
            public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
            public void checkClientTrusted(X509Certificate[] chain, String type) throws CertificateException {
                throw new CertificateException("Not a server");
            }
            public void checkServerTrusted(X509Certificate[] chain, String type) throws CertificateException {
                try {
                    if (chain.length != 0 && targetId.equals(fingerprint(chain[0]))) return;
                } catch (Exception e) { throw new CertificateException(e); }
                throw new CertificateException("Discovered host certificate changed");
            }
        };
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, new TrustManager[]{verifier}, null);
        transport.connect(new InetSocketAddress(host, port - 8), 4000);
        transport.setSoTimeout(5000);
        try (SSLSocket tls = (SSLSocket)context.getSocketFactory().createSocket(transport, host, port - 8, true)) {
            tls.startHandshake();
            X509Certificate leaf = (X509Certificate)tls.getSession().getPeerCertificates()[0];
            if (!targetId.equals(fingerprint(leaf))) throw new CertificateException("Host certificate changed");
            String message = "FLASHLINK_PAIR/1 " + token + " " + pin + " " + deviceName() + "\n";
            tls.getOutputStream().write(message.getBytes(StandardCharsets.UTF_8));
            tls.getOutputStream().flush();
            java.io.InputStream in = tls.getInputStream();
            if (in.read() != 'O' || in.read() != 'K' || in.read() != '\n') {
                throw new IOException("电脑未接受配对请求，请稍后重试。");
            }
            return leaf;
        } finally { close(); }
    }
}
