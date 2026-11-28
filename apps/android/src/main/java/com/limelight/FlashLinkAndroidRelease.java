// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.content.Context;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.Signature;
import android.os.Build;
import android.util.Base64;

import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InterruptedIOException;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.security.KeyFactory;
import java.security.MessageDigest;
import java.security.spec.X509EncodedKeySpec;
import java.util.Arrays;
import java.util.concurrent.TimeUnit;

import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;

/** Authenticates the release metadata before using its download URL or displaying its notes. */
final class FlashLinkAndroidRelease {
    static final String PACKAGE = "io.github.ricardomin.flashlink";
    private static final String REPOSITORY = "https://github.com/RicardoMin/FlashLink/releases/";
    private static final int MAX_ENVELOPE = 128 * 1024;
    private static final int MAX_APK = 512 * 1024 * 1024;
    private static final OkHttpClient HTTP = new OkHttpClient.Builder()
            .connectTimeout(8, TimeUnit.SECONDS).readTimeout(20, TimeUnit.SECONDS)
            .followRedirects(false).build();

    final String version, notes;
    final long build;
    private final String url, sha256;
    private final long size;
    final byte[] envelope;

    private FlashLinkAndroidRelease(byte[] envelope, JSONObject android) throws Exception {
        this.envelope = envelope;
        version = android.getString("version");
        require(version.matches("(0|[1-9][0-9]{0,8})(\\.(0|[1-9][0-9]{0,8})){2}"));
        build = number(android, "build", Integer.MAX_VALUE);
        notes = android.getString("notes");
        require(!notes.trim().isEmpty() && notes.getBytes(StandardCharsets.UTF_8).length <= 4096);
        JSONObject apk = android.getJSONObject("assets").getJSONObject("apk");
        url = apk.getString("url");
        require(url.equals(REPOSITORY + "download/v" + version + "/FlashLink-Android-arm64.apk"));
        size = number(apk, "size", MAX_APK);
        sha256 = apk.getString("sha256");
        require(sha256.matches("[0-9a-f]{64}"));
    }

    private static long number(JSONObject json, String name, long maximum) throws Exception {
        Object value = json.get(name);
        require(value instanceof Integer || value instanceof Long);
        long result = ((Number)value).longValue();
        require(result > 0 && result <= maximum);
        return result;
    }

    static FlashLinkAndroidRelease check(Context context) throws Exception {
        try (Response response = open(REPOSITORY + "latest/download/flashlink-update.json", 30)) {
            return parse(context, bytes(response.body().byteStream(), MAX_ENVELOPE));
        }
    }

    static FlashLinkAndroidRelease parse(Context context, byte[] envelope) throws Exception {
        require(envelope != null && envelope.length <= MAX_ENVELOPE);
        JSONObject wrapper = new JSONObject(new String(envelope, StandardCharsets.UTF_8));
        byte[] payload = Base64.decode(wrapper.getString("payload"), Base64.DEFAULT);
        java.security.Signature signature = java.security.Signature.getInstance("SHA256withRSA");
        try (InputStream input = context.getAssets().open("flashlink-update-key.der")) {
            signature.initVerify(KeyFactory.getInstance("RSA").generatePublic(
                    new X509EncodedKeySpec(bytes(input, 8192))));
        }
        signature.update(payload);
        require(signature.verify(Base64.decode(wrapper.getString("signature"), Base64.DEFAULT)));
        JSONObject manifest = new JSONObject(new String(payload, StandardCharsets.UTF_8));
        require(number(manifest, "schema", 1) == 1);
        return new FlashLinkAndroidRelease(envelope, manifest.getJSONObject("android"));
    }

    private static byte[] bytes(InputStream input, int maxSize) throws IOException {
        ByteArrayOutputStream result = new ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        int count;
        while ((count = input.read(buffer)) != -1) {
            require(count <= maxSize - result.size());
            result.write(buffer, 0, count);
        }
        return result.toByteArray();
    }

    boolean newerThan(PackageInfo installed) throws IOException {
        if (build <= buildOf(installed)) return false;
        String[] target = version.split("\\."), current = installed.versionName.split("\\.");
        require(current.length == 3);
        for (int i = 0; i < current.length; i++) {
            int difference = Integer.compare(Integer.parseInt(target[i]), Integer.parseInt(current[i]));
            require(difference >= 0);
            if (difference > 0) break;
        }
        return true;
    }

    private static Response open(String address, int timeoutSeconds) throws IOException {
        OkHttpClient client = HTTP.newBuilder().callTimeout(timeoutSeconds, TimeUnit.SECONDS).build();
        URL next = new URL(address);
        for (int redirects = 0; redirects <= 5; redirects++) {
            String host = next.getHost();
            require(next.getProtocol().equals("https") && next.getUserInfo() == null &&
                    (next.getPort() == -1 || next.getPort() == 443) &&
                    (host.equals("github.com") || host.endsWith(".githubusercontent.com")));
            Response response = client.newCall(new Request.Builder().url(next).build()).execute();
            if (response.code() == 200 && response.body() != null) return response;
            try (Response ignored = response) {
                String location = response.header("Location");
                require(response.isRedirect() && location != null);
                next = new URL(next, location);
            }
        }
        throw new IOException("Too many redirects");
    }

    interface Progress { void changed(int percent); }

    File prepareApk(Context context, boolean cachedOnly, Progress progress) throws Exception {
        PackageManager manager = context.getPackageManager();
        int flags = Build.VERSION.SDK_INT >= 28 ? PackageManager.GET_SIGNING_CERTIFICATES : PackageManager.GET_SIGNATURES;
        PackageInfo installed = manager.getPackageInfo(context.getPackageName(), flags);
        require(PACKAGE.equals(installed.packageName) && newerThan(installed));
        File directory = new File(context.getCacheDir(), "flashlink-updates");
        require(directory.isDirectory() || directory.mkdir() || directory.isDirectory());
        File published = new File(directory, sha256 + ".apk");
        if (published.isFile() && published.length() == size) {
            try (InputStream in = new FileInputStream(published)) {
                if (matches(in, null, progress)) {
                    checkPackage(manager, installed, flags, published);
                    return published;
                }
            }
        }
        require(!cachedOnly);
        // Keep verified APKs available to the installer while removing superseded downloads.
        File[] files = directory.listFiles();
        if (files != null) {
            long cutoff = System.currentTimeMillis() - TimeUnit.DAYS.toMillis(1);
            for (File file : files) {
                if (file.lastModified() < cutoff && file.getName().endsWith(".apk")) file.delete();
            }
        }
        File staging = File.createTempFile("download-", ".apk", directory);
        try {
            try (Response response = open(url, 10 * 60); FileOutputStream sink = new FileOutputStream(staging)) {
                long length = response.body().contentLength();
                require(length < 0 || length == size);
                require(matches(response.body().byteStream(), sink, progress));
                sink.getFD().sync();
            }
            checkPackage(manager, installed, flags, staging);
            require(staging.renameTo(published));
            return published;
        } finally {
            staging.delete();
        }
    }

    private boolean matches(InputStream in, FileOutputStream out, Progress progress) throws Exception {
        MessageDigest hash = MessageDigest.getInstance("SHA-256");
        byte[] block = new byte[65536];
        long total = 0;
        int length, previous = -1;
        while ((length = in.read(block)) != -1) {
            if (Thread.currentThread().isInterrupted()) throw new InterruptedIOException();
            total += length;
            require(total <= size);
            hash.update(block, 0, length);
            if (out != null) out.write(block, 0, length);
            int percent = (int)(100 * total / size);
            if (percent != previous) progress.changed(percent);
            previous = percent;
        }
        byte[] actual = hash.digest();
        for (int i = 0; i < actual.length; i++) {
            if ((actual[i] & 0xff) != Integer.parseInt(sha256.substring(2*i, 2*i+2), 16)) return false;
        }
        return total == size;
    }

    private void checkPackage(PackageManager manager, PackageInfo installed, int flags, File file) throws IOException {
        PackageInfo apk = manager.getPackageArchiveInfo(file.getPath(), flags);
        require(apk != null && PACKAGE.equals(apk.packageName) && buildOf(apk) == build &&
                version.equals(apk.versionName));
        Signature[] a = signers(installed), b = signers(apk);
        require(a != null && b != null && a.length == 1 && Arrays.equals(a, b));
    }

    private static Signature[] signers(PackageInfo info) {
        if (Build.VERSION.SDK_INT >= 28) {
            return info.signingInfo == null ? null : info.signingInfo.getApkContentsSigners();
        }
        return info.signatures;
    }

    private static long buildOf(PackageInfo info) {
        return Build.VERSION.SDK_INT >= 28 ? info.getLongVersionCode() : info.versionCode;
    }

    private static void require(boolean accepted) throws IOException {
        if (!accepted) throw new IOException("Invalid FlashLink release");
    }
}
