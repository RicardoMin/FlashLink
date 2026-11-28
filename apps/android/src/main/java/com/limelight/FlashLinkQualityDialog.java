// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.app.AlertDialog;
import android.content.Context;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.Display;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Spinner;
import android.widget.TextView;

import com.limelight.preferences.PreferenceConfiguration;

import java.math.BigDecimal;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.TreeSet;
import java.util.function.Supplier;

/** Edits a copy of the active stream settings until the user applies them. */
final class FlashLinkQualityDialog {
    private final FlashLinkGame game;
    private final FlashLinkQuality active;
    private final AlertDialog.Builder builder;
    private final LinearLayout form;

    FlashLinkQualityDialog(FlashLinkGame game, FlashLinkQuality active) {
        this.game = game;
        this.active = active;
        builder = new AlertDialog.Builder(game);
        form = new LinearLayout(builder.getContext());
        form.setOrientation(LinearLayout.VERTICAL);
        form.setPadding(dp(20), dp(4), dp(20), 0);
    }

    private List<Integer> frameRates() {
        Display display = game.getWindowManager().getDefaultDisplay();
        float maxRefresh = display.getRefreshRate();
        for (Display.Mode mode : display.getSupportedModes()) {
            maxRefresh = Math.max(maxRefresh, mode.getRefreshRate());
        }
        boolean unlocked = PreferenceConfiguration.readPreferences(game).unlockFps;
        TreeSet<Integer> rates = new TreeSet<>();
        for (String entry : game.getResources().getStringArray(R.array.fps_values)) {
            int fps = Integer.parseInt(entry);
            if (unlocked || fps <= 60 || fps <= maxRefresh + 2) rates.add(fps);
        }
        if (maxRefresh > 0) rates.add(Math.round(maxRefresh));
        rates.add(active.fps);
        return new ArrayList<>(rates);
    }

    void show() {
        String currentSize = active.width + "x" + active.height;
        List<String> sizes = new ArrayList<>(Arrays.asList(game.getResources().getStringArray(R.array.resolution_values)));
        Display.Mode mode = game.getWindowManager().getDefaultDisplay().getMode();
        int longEdge = Math.max(mode.getPhysicalWidth(), mode.getPhysicalHeight());
        int shortEdge = Math.min(mode.getPhysicalWidth(), mode.getPhysicalHeight());
        String nativeSize = longEdge + "x" + shortEdge;
        if (!sizes.contains(nativeSize)) sizes.add(nativeSize);
        if (!sizes.contains(currentSize)) sizes.add(currentSize);
        List<String> labels = new ArrayList<>();
        for (String size : sizes) labels.add(size.replace("x", " × "));
        Spinner resolution = selector("分辨率", labels, sizes.indexOf(currentSize));

        List<Integer> rates = frameRates();
        labels = new ArrayList<>();
        for (int rate : rates) labels.add(rate + " FPS");
        Spinner fps = selector("帧率", labels, rates.indexOf(active.fps));

        TreeSet<Integer> presets = new TreeSet<>(Arrays.asList(500, 1000, 3000, 5000, 10000, 15000,
                20000, 30000, 50000, 80000, 100000, 150000));
        presets.add(active.bitrateKbps);
        List<Integer> bitrates = new ArrayList<>(presets);
        labels = new ArrayList<>();
        for (int kbps : bitrates) labels.add(FlashLinkQuality.mbps(kbps) + " Mbps");
        labels.add("自定义…");
        Spinner bitrate = selector("目标码率", labels, bitrates.indexOf(active.bitrateKbps));

        EditText custom = new EditText(builder.getContext());
        custom.setSingleLine();
        custom.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        custom.setHint("0.5–150 Mbps");
        custom.setContentDescription("自定义目标码率（Mbps）");
        custom.setText(FlashLinkQuality.mbps(active.bitrateKbps));
        custom.setSelectAllOnFocus(true);
        custom.setVisibility(View.GONE);
        form.addView(custom);
        TextView reason = new TextView(builder.getContext());
        form.addView(reason);

        ScrollView scroll = new ScrollView(builder.getContext());
        scroll.addView(form);
        AlertDialog dialog = builder.setTitle("串流画质").setView(scroll)
                .setNegativeButton("取消", null).setPositiveButton("应用", null).create();
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN |
                WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        dialog.show();
        Supplier<FlashLinkQuality> selection = () -> {
            int index = bitrate.getSelectedItemPosition();
            int kbps;
            if (index == bitrates.size()) {
                try {
                    kbps = new BigDecimal(custom.getText().toString().trim().replace(',', '.'))
                            .movePointRight(3).intValueExact();
                    if (!FlashLinkLiveSetting.Kind.BITRATE.valid(kbps)) return null;
                } catch (ArithmeticException | NumberFormatException invalid) {
                    return null;
                }
            } else {
                kbps = bitrates.get(index);
            }
            String[] size = sizes.get(resolution.getSelectedItemPosition()).split("x");
            return new FlashLinkQuality(Integer.parseInt(size[0]), Integer.parseInt(size[1]),
                    rates.get(fps.getSelectedItemPosition()), kbps);
        };
        Runnable updateAction = () -> {
            FlashLinkQuality target = selection.get();
            String message = target == null ? "请输入 0.5–150 Mbps，最多三位小数" : game.liveQualityRestriction(target);
            reason.setText(message);
            reason.setVisibility(message == null ? View.GONE : View.VISIBLE);
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setEnabled(message == null && !active.equals(target));
        };
        AdapterView.OnItemSelectedListener changed = new AdapterView.OnItemSelectedListener() {
            @Override public void onItemSelected(AdapterView<?> parent, View view, int index, long id) {
                custom.setVisibility(bitrate.getSelectedItemPosition() == bitrates.size() ? View.VISIBLE : View.GONE);
                updateAction.run();
            }
            @Override public void onNothingSelected(AdapterView<?> parent) {}
        };
        resolution.setOnItemSelectedListener(changed);
        fps.setOnItemSelectedListener(changed);
        bitrate.setOnItemSelectedListener(changed);
        custom.addTextChangedListener(new TextWatcher() {
            @Override public void afterTextChanged(Editable text) { updateAction.run(); }
            @Override public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
            @Override public void onTextChanged(CharSequence s, int start, int before, int count) {}
        });
        updateAction.run();
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
            FlashLinkQuality selected = selection.get();
            if (selected == null) {
                custom.setError("请输入 0.5–150 Mbps，最多三位小数");
                return;
            }
            dialog.dismiss();
            game.applyQuality(selected);
        });
    }

    private Spinner selector(String name, List<String> choices, int selected) {
        Context context = builder.getContext();
        LinearLayout row = new LinearLayout(context);
        row.setGravity(Gravity.CENTER_VERTICAL);
        form.addView(row);
        TextView label = new TextView(context);
        label.setText(name);
        label.setTextSize(16);
        row.addView(label, new LinearLayout.LayoutParams(dp(80), -2));
        Spinner spinner = new Spinner(context);
        spinner.setId(View.generateViewId());
        label.setLabelFor(spinner.getId());
        spinner.setContentDescription(name);
        ArrayAdapter<String> adapter = new ArrayAdapter<>(context, android.R.layout.simple_spinner_item, choices);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        spinner.setAdapter(adapter);
        spinner.setSelection(selected);
        row.addView(spinner, new LinearLayout.LayoutParams(0, dp(48), 1f));
        return spinner;
    }

    private int dp(int value) {
        return Math.round(value * game.getResources().getDisplayMetrics().density);
    }
}
