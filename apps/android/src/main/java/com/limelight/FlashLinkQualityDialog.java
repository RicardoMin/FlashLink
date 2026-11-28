// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.app.AlertDialog;
import android.content.Context;
import android.content.res.ColorStateList;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.graphics.drawable.StateListDrawable;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.Display;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputMethodManager;
import android.widget.AdapterView;
import android.widget.ArrayAdapter;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.RadioButton;
import android.widget.RadioGroup;
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
    private static final int BACKGROUND = 0xff192635, SURFACE = 0xff2b3b4d, SELECTED = 0xff284f70;
    private static final int TEXT = 0xffedf4fa, MUTED = 0xffbdcddc, ACCENT = 0xffa5d9ff;
    private final FlashLinkGame game;
    private final FlashLinkQuality active;
    private final AlertDialog.Builder builder;
    private final LinearLayout form;

    FlashLinkQualityDialog(FlashLinkGame game, FlashLinkQuality active) {
        this.game = game;
        this.active = active;
        builder = new AlertDialog.Builder(game, android.R.style.Theme_Material_Dialog_Alert);
        form = new LinearLayout(builder.getContext());
        form.setOrientation(LinearLayout.VERTICAL);
        form.setPadding(dp(24), dp(12), dp(24), 0);
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
        RadioGroup quality = qualityOptions();
        EditText custom = new EditText(builder.getContext());
        custom.setSingleLine();
        custom.setInputType(InputType.TYPE_CLASS_NUMBER | InputType.TYPE_NUMBER_FLAG_DECIMAL);
        custom.setImeOptions(EditorInfo.IME_ACTION_DONE | EditorInfo.IME_FLAG_NO_EXTRACT_UI);
        custom.setHint("0.5–150 Mbps");
        custom.setContentDescription("自定义码率，单位 Mbps");
        custom.setText(FlashLinkQuality.mbps(active.bitrateKbps));
        custom.setSelectAllOnFocus(true);
        LinearLayout customRow = row("码率 (Mbps)", custom);
        customRow.setVisibility(active.preset() == FlashLinkQuality.Preset.CUSTOM ? View.VISIBLE : View.GONE);

        String currentSize = active.width + "x" + active.height;
        List<String> sizes = new ArrayList<>(Arrays.asList(game.getResources().getStringArray(R.array.resolution_values)));
        Display.Mode mode = game.getWindowManager().getDefaultDisplay().getMode();
        int longEdge = Math.max(mode.getPhysicalWidth(), mode.getPhysicalHeight());
        int shortEdge = Math.min(mode.getPhysicalWidth(), mode.getPhysicalHeight());
        String nativeSize = longEdge + "x" + shortEdge;
        if (!sizes.contains(nativeSize)) sizes.add(nativeSize);
        if (!sizes.contains(currentSize)) sizes.add(currentSize);
        List<String> labels = new ArrayList<>();
        for (String size : sizes) {
            labels.add(size.replace("x", " × ") + (size.equals(nativeSize) ? "（本机）" : ""));
        }
        Spinner resolution = selector("分辨率", labels, sizes.indexOf(currentSize));

        List<Integer> rates = frameRates();
        labels = new ArrayList<>();
        for (int rate : rates) labels.add(rate + "帧");
        Spinner fps = selector("帧率", labels, rates.indexOf(active.fps));

        TextView reason = new TextView(builder.getContext());
        reason.setTextColor(MUTED);
        reason.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
        form.addView(reason);

        ScrollView scroll = new ScrollView(builder.getContext());
        scroll.addView(form);
        AlertDialog dialog = builder.setTitle("画面设置").setView(scroll)
                .setNegativeButton("取消", null).setPositiveButton("应用", null).create();
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN |
                WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE);
        dialog.show();
        dialog.getWindow().setBackgroundDrawable(shape(BACKGROUND, 18));
        int width = Math.min(dp(408), game.getWindow().getDecorView().getWidth() - dp(24));
        if (width > 0) dialog.getWindow().setLayout(width, WindowManager.LayoutParams.WRAP_CONTENT);
        dialog.getButton(AlertDialog.BUTTON_NEGATIVE).setTextColor(MUTED);
        dialog.getButton(AlertDialog.BUTTON_POSITIVE).setTextColor(new ColorStateList(
                new int[][]{{-android.R.attr.state_enabled}, {}}, new int[]{0xff8493a0, ACCENT}));
        Supplier<FlashLinkQuality> selection = () -> {
            RadioButton checked = quality.findViewById(quality.getCheckedRadioButtonId());
            FlashLinkQuality.Preset preset = (FlashLinkQuality.Preset) checked.getTag();
            int kbps = preset.kbps;
            if (preset == FlashLinkQuality.Preset.CUSTOM) {
                try {
                    kbps = new BigDecimal(custom.getText().toString().trim().replace(',', '.'))
                            .movePointRight(3).intValueExact();
                    if (!FlashLinkLiveSetting.Kind.BITRATE.valid(kbps)) return null;
                } catch (ArithmeticException | NumberFormatException invalid) {
                    return null;
                }
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
                updateAction.run();
            }
            @Override public void onNothingSelected(AdapterView<?> parent) {}
        };
        resolution.setOnItemSelectedListener(changed);
        fps.setOnItemSelectedListener(changed);
        quality.setOnCheckedChangeListener((group, id) -> {
            boolean customSelected = group.findViewById(id).getTag() == FlashLinkQuality.Preset.CUSTOM;
            if (!customSelected && custom.hasFocus()) {
                game.getSystemService(InputMethodManager.class).hideSoftInputFromWindow(custom.getWindowToken(), 0);
                custom.clearFocus();
            }
            customRow.setVisibility(customSelected ? View.VISIBLE : View.GONE);
            updateAction.run();
        });
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

    private RadioGroup qualityOptions() {
        Context context = builder.getContext();
        TextView title = new TextView(context);
        title.setText("画质");
        title.setTextColor(MUTED);
        title.setTextSize(14);
        form.addView(title);
        RadioGroup choices = new RadioGroup(context);
        choices.setId(View.generateViewId());
        title.setLabelFor(choices.getId());
        choices.setOrientation(LinearLayout.HORIZONTAL);
        LinearLayout.LayoutParams size = new LinearLayout.LayoutParams(-1, -2);
        size.setMargins(0, dp(6), 0, dp(6));
        form.addView(choices, size);
        for (FlashLinkQuality.Preset preset : FlashLinkQuality.Preset.values()) {
            RadioButton chip = new RadioButton(context);
            chip.setId(View.generateViewId());
            chip.setTag(preset);
            chip.setText(preset.label);
            chip.setButtonDrawable(null);
            chip.setPadding(0, dp(4), 0, dp(4));
            chip.setGravity(Gravity.CENTER);
            chip.setMinimumHeight(dp(48));
            chip.setTextColor(TEXT);
            chip.setTextSize(15);
            StateListDrawable background = new StateListDrawable();
            GradientDrawable selected = shape(SELECTED, 10);
            selected.setStroke(dp(1), ACCENT);
            background.addState(new int[]{android.R.attr.state_checked}, selected);
            background.addState(new int[]{}, shape(SURFACE, 10));
            chip.setBackground(new RippleDrawable(ColorStateList.valueOf(0x308ecfff), background, null));
            RadioGroup.LayoutParams params = new RadioGroup.LayoutParams(0, -2, 1);
            if (choices.getChildCount() > 0) params.setMarginStart(dp(4));
            choices.addView(chip, params);
            if (active.preset() == preset) choices.check(chip.getId());
        }
        return choices;
    }

    private GradientDrawable shape(int color, int radius) {
        GradientDrawable shape = new GradientDrawable();
        shape.setColor(color);
        shape.setCornerRadius(dp(radius));
        return shape;
    }

    private LinearLayout row(String name, View control) {
        Context context = builder.getContext();
        LinearLayout row = new LinearLayout(context);
        row.setGravity(Gravity.CENTER_VERTICAL);
        form.addView(row);
        TextView label = new TextView(context);
        label.setText(name);
        label.setTextColor(MUTED);
        label.setTextSize(14);
        row.addView(label, new LinearLayout.LayoutParams(dp(86), -2));
        control.setId(View.generateViewId());
        label.setLabelFor(control.getId());
        control.setMinimumHeight(dp(48));
        row.addView(control, new LinearLayout.LayoutParams(0, -2, 1f));
        return row;
    }

    private Spinner selector(String name, List<String> choices, int selected) {
        Context context = builder.getContext();
        Spinner spinner = new Spinner(context, Spinner.MODE_DROPDOWN);
        spinner.setContentDescription(name);
        ArrayAdapter<String> adapter = new ArrayAdapter<>(context, android.R.layout.simple_spinner_item, choices);
        adapter.setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item);
        spinner.setAdapter(adapter);
        spinner.setSelection(selected);
        row(name, spinner);
        return spinner;
    }

    private int dp(int value) {
        return Math.round(value * game.getResources().getDisplayMetrics().density);
    }
}
