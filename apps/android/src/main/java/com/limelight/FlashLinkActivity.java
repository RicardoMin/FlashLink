// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Bundle;
import android.preference.PreferenceManager;
import android.view.Display;

import com.limelight.preferences.StreamSettings;
import io.flutter.embedding.android.FlutterActivity;
import io.flutter.embedding.engine.FlutterEngine;
import io.flutter.plugin.common.MethodChannel;

import java.net.URI;
import java.net.URISyntaxException;
import java.util.Collections;
import java.util.UUID;

/** Flutter owns discovery and selection; the native session pairs and opens the advertised PC. */
public final class FlashLinkActivity extends FlutterActivity {
    static final String HOST = "dev.flashlink.host";
    static final String PORT = "dev.flashlink.port";
    static final String ID = "dev.flashlink.peerId";
    static final String NAME = "dev.flashlink.peerName";
    private static final int PLAYER_REQUEST = 43;
    private static final String SAVED_ATTEMPT = "dev.flashlink.attempt";
    private static final String SAVED_PROCESS = "dev.flashlink.process";
    private static final String PROCESS_TOKEN = UUID.randomUUID().toString();
    private MethodChannel.Result pendingLaunch;
    private long attemptId;

    private static native void releaseRecoveredAttempt(long id);

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        if (savedInstanceState != null && PROCESS_TOKEN.equals(savedInstanceState.getString(SAVED_PROCESS))) {
            attemptId = savedInstanceState.getLong(SAVED_ATTEMPT);
        }
        super.onCreate(savedInstanceState);
        seedDisplayResolution();
    }

    private void seedDisplayResolution() {
        // Moonlight reads this key in PreferenceConfiguration and its settings screen.
        SharedPreferences settings = PreferenceManager.getDefaultSharedPreferences(this);
        if (settings.contains("list_resolution")) {
            return;
        }
        Display.Mode panel = getWindowManager().getDefaultDisplay().getMode();
        int longEdge = Math.max(panel.getPhysicalWidth(), panel.getPhysicalHeight());
        int shortEdge = Math.min(panel.getPhysicalWidth(), panel.getPhysicalHeight());
        String resolution = "1280x720";
        if (longEdge >= 2560 && shortEdge >= 1440) {
            resolution = "2560x1440";
        } else if (longEdge >= 1920 && shortEdge >= 1080) {
            resolution = "1920x1080";
        }
        settings.edit().putString("list_resolution", resolution).apply();
    }

    @Override
    protected void onSaveInstanceState(Bundle state) {
        super.onSaveInstanceState(state);
        state.putLong(SAVED_ATTEMPT, attemptId);
        state.putString(SAVED_PROCESS, PROCESS_TOKEN);
    }

    @Override
    public void configureFlutterEngine(FlutterEngine engine) {
        super.configureFlutterEngine(engine);
        MethodChannel channel = new MethodChannel(
                engine.getDartExecutor().getBinaryMessenger(), "dev.flashlink/player");
        channel.setMethodCallHandler((call, result) -> {
            if (call.method.equals("discover")) {
                new Thread(() -> {
                    try {
                        java.util.List<java.util.Map<String, String>> devices = FlashLinkDiscovery.scan(this);
                        runOnUiThread(() -> result.success(devices));
                    } catch (Exception e) {
                        runOnUiThread(() -> result.error("discovery_failed", "无法搜索附近电脑，请检查局域网连接。", null));
                    }
                }, "FlashLink LAN discovery").start();
                return;
            }
            if (call.method.equals("settings")) {
                if (attemptId != 0) {
                    result.error("busy", "Close the player before changing settings.", null);
                    return;
                }
                try {
                    startActivity(new Intent(this, StreamSettings.class));
                    result.success(null);
                } catch (android.content.ActivityNotFoundException e) {
                    result.error("settings_failed", e.getMessage(), null);
                }
                return;
            }
            if (!call.method.equals("launch")) {
                result.notImplemented();
                return;
            }
            if (attemptId != 0) {
                result.error("busy", "A Moonlight session is already open.", null);
                return;
            }
            String host = call.argument("host");
            String id = call.argument("id");
            String name = call.argument("name");
            Number attempt = call.argument("attemptId");
            try {
                if (host == null || attempt == null || attempt.longValue() <= 0 ||
                        id == null || !id.matches("[a-f0-9]{64}") || name == null || name.isEmpty()) {
                    throw new IllegalArgumentException("请选择已发现的 FlashLink 电脑。");
                }
                URI address = parseAddress(host);
                Intent intent = new Intent(this, FlashLinkSessionActivity.class);
                intent.putExtra(HOST, address.getHost());
                intent.putExtra(ID, id);
                intent.putExtra(NAME, name);
                intent.putExtra(PORT, address.getPort() < 0 ? 47989 : address.getPort());
                pendingLaunch = result;
                attemptId = attempt.longValue();
                startActivityForResult(intent, PLAYER_REQUEST);
            } catch (IllegalArgumentException | android.content.ActivityNotFoundException e) {
                pendingLaunch = null;
                attemptId = 0;
                result.error("launch_failed", e.getMessage(), null);
                return;
            }
            channel.invokeMethod("opened", Collections.singletonMap("attemptId", attempt));
        });
    }

    static URI parseAddress(String host) {
        String value = host.trim();
        if (value.indexOf(':') != value.lastIndexOf(':') && !value.startsWith("[")) {
            value = "[" + value + "]";
        }
        try {
            URI uri = new URI("moonlight://" + value);
            if (uri.getHost() != null && uri.getRawUserInfo() == null
                    && uri.getRawPath().isEmpty() && uri.getRawQuery() == null
                    && uri.getRawFragment() == null && uri.getPort() != 0
                    && uri.getPort() <= 65535 && !value.endsWith(":")) {
                return uri;
            }
        } catch (URISyntaxException ignored) {
            // Return the same actionable message for invalid hostnames and invalid ports.
        }
        throw new IllegalArgumentException("电脑返回的地址无效，请刷新设备列表。");
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != PLAYER_REQUEST || attemptId == 0) {
            return;
        }
        long returnedAttempt = attemptId;
        MethodChannel.Result result = pendingLaunch;
        attemptId = 0;
        pendingLaunch = null;
        if (result != null) {
            result.success(null);
        } else {
            // A recreated Flutter engine no longer owns the original method call.
            System.loadLibrary("flashlink_client");
            releaseRecoveredAttempt(returnedAttempt);
        }
    }
}
