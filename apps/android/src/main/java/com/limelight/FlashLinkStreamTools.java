// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.app.AlertDialog;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.RippleDrawable;
import android.os.SystemClock;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowManager;
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.Switch;
import android.widget.TextView;

import com.limelight.binding.input.evdev.EvdevListener;

/** In-session controls, opened from a single docked button. */
final class FlashLinkStreamTools {
    private static final int WRAP = -2, MATCH = -1;
    private static final int INK = 0xfff2f6fd, MUTED = 0xffbfcee0, ACCENT = 0xffa6cdff;
    private static final int TILE = 0xff293749;
    private enum Page { HOME, MOUSE, KEYS }
    private final FlashLinkGame game;
    private final FlashLinkControlDock layer;
    private final LinearLayout panel;
    private final ScrollView sheet;
    private TextView resetButton, dragButton;
    private boolean zoomed, drag;
    private float lastX, lastY;
    private int trackingPointer = -1;

    FlashLinkStreamTools(FlashLinkGame game) {
        this.game = game;
        panel = new LinearLayout(game);
        panel.setOrientation(LinearLayout.VERTICAL);
        panel.setPadding(dp(12), dp(6), dp(12), dp(6));
        sheet = new ScrollView(game);
        sheet.setBackground(shape(0xf51a2433, 18));
        sheet.setClipToOutline(true);
        sheet.setFillViewport(false);
        sheet.setClickable(true);
        sheet.addView(panel);
        layer = new FlashLinkControlDock(game, sheet, () -> {
            if (!collapse()) open(Page.HOME);
        }, this::collapse);
    }

    void attach() {
        game.addContentView(layer, new FrameLayout.LayoutParams(MATCH, MATCH));
    }

    void showViewReset(boolean zoomed) {
        this.zoomed = zoomed;
        if (resetButton != null) {
            resetButton.setEnabled(zoomed);
            resetButton.setAlpha(zoomed ? 1f : 0.45f);
        }
    }

    private void open(Page page) {
        game.cancelTouchscreenInput();
        release();
        panel.removeAllViews();
        resetButton = null;
        LinearLayout header = row();
        if (page != Page.HOME) {
            TextView back = button("‹", () -> open(Page.HOME));
            back.setTextSize(24);
            back.setContentDescription("返回控制中心");
            header.addView(back, new LinearLayout.LayoutParams(dp(48), dp(48)));
        }
        TextView title = label(page == Page.MOUSE ? "虚拟鼠标" : page == Page.KEYS ? "快捷键" : "控制中心", 18, INK);
        title.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        title.setPadding(dp(6), 0, 0, 0);
        header.addView(title, new LinearLayout.LayoutParams(WRAP, dp(48)));
        if (page == Page.HOME) homePanel();
        else if (page == Page.MOUSE) mousePanel();
        else keyPanel();
        layer.setPanelOpen(true);
        sheet.scrollTo(0, 0);
    }

    boolean collapse() {
        if (sheet.getVisibility() != View.VISIBLE) return false;
        release();
        layer.setPanelOpen(false);
        return true;
    }

    void setVisible(boolean visible) {
        if (!visible) collapse();
        layer.setVisibility(visible ? View.VISIBLE : View.GONE);
    }

    void release() {
        trackingPointer = -1;
        if (drag) {
            game.mouseButtonEvent(EvdevListener.BUTTON_LEFT, false);
            drag = false;
            dragButton.setText("拖动");
        }
    }

    private void homePanel() {
        LinearLayout first = row();
        primary(first, "触控模式", game.isTouchscreenTrackpad() ? "触控板" : "直接触控", R.drawable.fl_touch, () -> {
            game.toggleTouchMode();
            collapse();
        });
        FlashLinkQuality quality = game.currentQuality();
        if (quality != null) primary(first, "画面设置", quality.presetLabel(), R.drawable.fl_display, () -> {
            collapse();
            game.showQualitySettings();
        });

        LinearLayout actions = row();
        tool(actions, "虚拟鼠标", R.drawable.fl_mouse, () -> open(Page.MOUSE));
        tool(actions, "快捷键", R.drawable.fl_keys, () -> open(Page.KEYS));
        tool(actions, "系统键盘", R.drawable.fl_keyboard, () -> { collapse(); game.toggleKeyboard(); });
        actions = row();
        tool(actions, "输入文字", R.drawable.fl_text, this::textDialog);
        tool(actions, "声音", R.drawable.fl_sound, () -> { collapse(); FlashLinkAudio.show(game); });
        resetButton = tool(actions, "复位视图", R.drawable.fl_fit, () -> { game.resetViewport(); collapse(); });
        showViewReset(zoomed);

        View divider = new View(game);
        divider.setBackgroundColor(0xff3b495a);
        LinearLayout.LayoutParams rule = new LinearLayout.LayoutParams(MATCH, dp(1));
        rule.setMargins(dp(3), dp(7), dp(3), 0);
        panel.addView(divider, rule);
        LinearLayout footer = row();
        Switch status = new Switch(game);
        status.setText("串流状态");
        status.setTextColor(INK);
        status.setTextSize(14);
        status.setPadding(dp(4), 0, dp(8), 0);
        status.setChecked(game.isStatusOverlayEnabled());
        status.setOnCheckedChangeListener((v, enabled) -> game.setStatusOverlayEnabled(enabled));
        footer.addView(status, new LinearLayout.LayoutParams(0, dp(48), 1f));
        TextView leave = button("断开连接", game::leaveStream);
        leave.setTextColor(0xffffb7b2);
        leave.setBackground(new RippleDrawable(ColorStateList.valueOf(0x33445566), null, shape(Color.WHITE, 8)));
        footer.addView(leave, new LinearLayout.LayoutParams(dp(76), dp(48)));
    }

    private void primary(LinearLayout row, String title, String value, int icon, Runnable action) {
        LinearLayout tile = new LinearLayout(game);
        tile.setOrientation(LinearLayout.VERTICAL);
        tile.setGravity(Gravity.CENTER_VERTICAL);
        tile.setPadding(dp(10), dp(6), dp(6), dp(6));
        tile.setBackground(tileBackground(0xff304861));
        tile.setFocusable(true);
        tile.setContentDescription(title + "，" + value);
        tile.setOnClickListener(v -> action.run());
        TextView name = label(title, 13, MUTED);
        name.setCompoundDrawablesRelative(icon(icon, 16, ACCENT), null, null, null);
        name.setCompoundDrawablePadding(dp(5));
        tile.addView(name);
        TextView selection = label(value, 16, INK);
        selection.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        tile.addView(selection);
        addCell(row, tile, 62);
    }

    private TextView tool(LinearLayout row, String title, int icon, Runnable action) {
        TextView view = button(title, action);
        view.setTextSize(12);
        view.setPadding(0, dp(7), 0, dp(5));
        view.setCompoundDrawables(null, icon(icon, 20, MUTED), null, null);
        view.setCompoundDrawablePadding(dp(2));
        addCell(row, view, 52);
        return view;
    }

    private Drawable icon(int resource, int size, int tint) {
        Drawable icon = game.getDrawable(resource).mutate();
        icon.setBounds(0, 0, dp(size), dp(size));
        icon.setTint(tint);
        return icon;
    }

    private void mousePanel() {
        TextView pad = button("触控板", () -> click(EvdevListener.BUTTON_LEFT));
        pad.setContentDescription("滑动以移动指针，轻点以单击");
        pad.setBackground(shape(0xff344556, 12));
        pad.setOnTouchListener(new View.OnTouchListener() {
            private float startX, startY;
            private boolean moved;
            @Override public boolean onTouch(View view, MotionEvent event) {
                int action = event.getActionMasked();
                if (action == MotionEvent.ACTION_DOWN) {
                    view.getParent().requestDisallowInterceptTouchEvent(true);
                    trackingPointer = event.getPointerId(0);
                    lastX = startX = event.getX();
                    lastY = startY = event.getY();
                    moved = false;
                } else if (action == MotionEvent.ACTION_MOVE) {
                    int i = event.findPointerIndex(trackingPointer);
                    if (i >= 0) {
                        float x = event.getX(i), y = event.getY(i);
                        if (!moved) moved = Math.hypot(x - startX, y - startY) > dp(3);
                        if (moved) {
                            int dx = Math.round(x - lastX), dy = Math.round(y - lastY);
                            if (dx != 0 || dy != 0) game.mouseMove(dx, dy);
                            lastX += dx;
                            lastY += dy;
                        }
                    }
                } else if (action == MotionEvent.ACTION_CANCEL) {
                    release();
                } else if ((action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_POINTER_UP)
                        && event.getPointerId(event.getActionIndex()) == trackingPointer) {
                    if (!moved && !drag) view.performClick();
                    trackingPointer = -1;
                }
                return true;
            }
        });
        panel.addView(pad, new LinearLayout.LayoutParams(MATCH, dp(90)));
        LinearLayout row = row();
        item(row, "左键", () -> click(EvdevListener.BUTTON_LEFT));
        item(row, "中键", () -> click(EvdevListener.BUTTON_MIDDLE));
        item(row, "右键", () -> click(EvdevListener.BUTTON_RIGHT));
        row = row();
        item(row, "上滚", () -> game.mouseVScroll((byte)1));
        item(row, "下滚", () -> game.mouseVScroll((byte)-1));
        dragButton = item(row, "拖动", () -> {
            drag = !drag;
            game.mouseButtonEvent(EvdevListener.BUTTON_LEFT, drag);
            dragButton.setText(drag ? "松开" : "拖动");
        });
    }

    private void click(int button) {
        release();
        game.mouseButtonEvent(button, true);
        game.mouseButtonEvent(button, false);
    }

    private void keyPanel() {
        LinearLayout row = row();
        item(row, "空格", () -> key(KeyEvent.KEYCODE_SPACE));
        item(row, "回车", () -> key(KeyEvent.KEYCODE_ENTER));
        item(row, "Esc", () -> key(KeyEvent.KEYCODE_ESCAPE));
        row = row();
        item(row, "上一页", () -> key(KeyEvent.KEYCODE_PAGE_UP));
        item(row, "下一页", () -> key(KeyEvent.KEYCODE_PAGE_DOWN));
    }

    private void key(int code) {
        game.keyboardEvent(true, (short)code);
        game.keyboardEvent(false, (short)code);
    }

    private void textDialog() {
        collapse();
        EditText entry = new EditText(game);
        entry.setHint("发送到电脑");
        entry.setSingleLine(true);
        entry.setImeOptions(EditorInfo.IME_ACTION_SEND);
        entry.setFilters(new android.text.InputFilter[]{new android.text.InputFilter.LengthFilter(1024)});
        AlertDialog dialog = new AlertDialog.Builder(game).setTitle("输入文字").setView(entry)
                .setNegativeButton("取消", null)
                .setPositiveButton("发送", (d, which) -> {
                    String value = entry.getText().toString();
                    if (!value.isEmpty() && !game.isFinishing()) {
                        game.onKeyMultiple(KeyEvent.KEYCODE_UNKNOWN, 1,
                                new KeyEvent(SystemClock.uptimeMillis(), value, 0, 0));
                    }
                }).create();
        entry.setOnEditorActionListener((v, action, key) -> {
            if (action != EditorInfo.IME_ACTION_SEND) return false;
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            return true;
        });
        dialog.getWindow().addFlags(WindowManager.LayoutParams.FLAG_SECURE);
        dialog.getWindow().setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_VISIBLE);
        dialog.show();
        entry.requestFocus();
    }

    private LinearLayout row() {
        LinearLayout row = new LinearLayout(game);
        panel.addView(row, new LinearLayout.LayoutParams(MATCH, WRAP));
        return row;
    }

    private TextView item(LinearLayout row, String label, Runnable action) {
        TextView view = button(label, action);
        addCell(row, view, 48);
        return view;
    }

    private void addCell(LinearLayout row, View view, int minimumHeight) {
        view.setMinimumHeight(dp(minimumHeight));
        LinearLayout.LayoutParams p = new LinearLayout.LayoutParams(0, WRAP, 1f);
        p.setMargins(dp(2), dp(2), dp(2), dp(2));
        row.addView(view, p);
    }

    private TextView label(String text, int size, int color) {
        TextView view = new TextView(game);
        view.setText(text);
        view.setGravity(Gravity.CENTER_VERTICAL);
        view.setTextColor(color);
        view.setTextSize(size);
        return view;
    }

    private TextView button(String text, Runnable action) {
        TextView view = label(text, 14, INK);
        view.setGravity(Gravity.CENTER);
        view.setBackground(tileBackground(TILE));
        view.setOnClickListener(v -> action.run());
        view.setFocusable(true);
        return view;
    }

    private RippleDrawable tileBackground(int color) {
        return new RippleDrawable(ColorStateList.valueOf(0x448cc2ff), shape(color, 10), null);
    }

    private GradientDrawable shape(int color, int radius) {
        GradientDrawable shape = new GradientDrawable();
        shape.setColor(color);
        shape.setCornerRadius(dp(radius));
        return shape;
    }

    private int dp(int size) {
        return Math.round(game.getResources().getDisplayMetrics().density * size);
    }
}
