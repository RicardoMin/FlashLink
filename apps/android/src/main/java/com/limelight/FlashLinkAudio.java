// SPDX-License-Identifier: MIT
package com.limelight;

import android.app.AlertDialog;
import android.content.Context;
import android.content.SharedPreferences;
import android.preference.ListPreference;
import android.preference.PreferenceCategory;
import android.preference.PreferenceManager;
import android.view.View;
import android.widget.LinearLayout;
import android.widget.RadioButton;
import android.widget.RadioGroup;
import android.widget.ScrollView;
import android.widget.TextView;

/** The settings screen and the player use the same two persisted choices. */
public final class FlashLinkAudio {
    static final String MANAGED = "dev.flashlink.managedAudio";
    private static final String ROUTE = "flashlink_audio_route";
    private static final String AFTER = "flashlink_audio_after";
    static final String CONTROLLER = "controller";
    static final String HOST = "host";
    private static final String BOTH = "both";
    private static final String[] ROUTES = {BOTH, CONTROLLER, HOST};
    private static final String[] DESTINATIONS = {"两端同时播放", "仅手机或平板播放", "仅电脑播放"};
    private static final String[] POLICIES = {"sync", "mute"};
    private static final String[] ACTIONS = {"跟随手机或平板的媒体音量", "保持电脑静音"};

    private FlashLinkAudio() { }

    static String route(Context context) {
        SharedPreferences prefs = PreferenceManager.getDefaultSharedPreferences(context);
        String value = prefs.getString(ROUTE, null);
        if (value == null) {
            // Honor the old "play audio on PC" setting when present. New installs play on both.
            return prefs.getBoolean("checkbox_host_audio", true) ? BOTH : CONTROLLER;
        }
        return CONTROLLER.equals(value) || HOST.equals(value) ? value : BOTH;
    }

    static String after(Context context) {
        String value = PreferenceManager.getDefaultSharedPreferences(context).getString(AFTER, "sync");
        return "mute".equals(value) ? "mute" : "sync";
    }

    public static void addPreferences(PreferenceCategory category) {
        addPreference(category, ROUTE, "声音播放位置", ROUTES, DESTINATIONS, route(category.getContext()));
        addPreference(category, AFTER, "断开后的电脑声音", POLICIES, ACTIONS, after(category.getContext()));
    }

    private static void addPreference(PreferenceCategory category, String key, String title,
                                      String[] values, String[] labels, String current) {
        ListPreference pref = new ListPreference(category.getContext());
        pref.setKey(key);
        pref.setTitle(title);
        pref.setDialogTitle(title);
        pref.setEntries(labels);
        pref.setEntryValues(values);
        pref.setDefaultValue(current);
        pref.setSummary("%s");
        category.addPreference(pref);
    }

    static void show(FlashLinkGame game) {
        if (!game.getIntent().getBooleanExtra(MANAGED, false)) {
            new AlertDialog.Builder(game).setTitle("串流声音")
                    .setMessage("当前连接使用兼容音频模式。请确认 Windows 主机已更新后重新连接，即可在串流中切换播放位置及设置断开后的电脑音量。")
                    .setPositiveButton("确定", null).show();
            return;
        }
        ScrollView scroll = new ScrollView(game);
        LinearLayout panel = new LinearLayout(game);
        panel.setOrientation(LinearLayout.VERTICAL);
        int pad = Math.round(20 * game.getResources().getDisplayMetrics().density);
        panel.setPadding(pad, pad / 2, pad, 0);
        scroll.addView(panel);
        RadioGroup route = group(panel, "声音播放位置", ROUTES, DESTINATIONS, route(game));
        RadioGroup after = group(panel, "断开后的电脑声音", POLICIES, ACTIONS, after(game));
        new AlertDialog.Builder(game).setTitle("串流声音").setView(scroll)
                .setNegativeButton("取消", null)
                .setPositiveButton("应用", (dialog, which) -> {
                    String playback = (String) route.findViewById(route.getCheckedRadioButtonId()).getTag();
                    String disconnect = (String) after.findViewById(after.getCheckedRadioButtonId()).getTag();
                    PreferenceManager.getDefaultSharedPreferences(game).edit()
                            .putString(ROUTE, playback).putString(AFTER, disconnect).apply();
                    game.audioChanged();
                }).show();
    }

    private static RadioGroup group(LinearLayout panel, String title, String[] values,
                                    String[] labels, String current) {
        TextView heading = new TextView(panel.getContext());
        heading.setText(title);
        panel.addView(heading);
        RadioGroup group = new RadioGroup(panel.getContext());
        for (int i = 0; i < values.length; i++) {
            RadioButton option = new RadioButton(panel.getContext());
            option.setId(View.generateViewId());
            option.setTag(values[i]);
            option.setText(labels[i]);
            group.addView(option);
            if (current.equals(values[i])) group.check(option.getId());
        }
        panel.addView(group);
        return group;
    }
}
