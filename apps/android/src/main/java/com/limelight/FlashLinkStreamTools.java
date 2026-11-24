// SPDX-License-Identifier: MIT
package com.limelight;

import android.app.AlertDialog;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
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
import android.widget.TextView;

import com.limelight.binding.input.evdev.EvdevListener;

/** Native controls overlay. The game surface remains responsible for every touch outside the controls. */
final class FlashLinkStreamTools {
    private static final int WRAP = -2, MATCH = -1;
    private final FlashLinkGame game;
    private final FrameLayout layer;
    private final LinearLayout panel;
    private final ScrollView sheet;
    private TextView mouseTab, modeTab, toolsTab, resetTab, dragButton;
    private boolean mouseOpen, drag;
    private float lastX, lastY;
    private int trackingPointer = -1;

    FlashLinkStreamTools(FlashLinkGame game) {
        this.game = game;
        layer = new FrameLayout(game);
        panel = new LinearLayout(game);
        panel.setOrientation(LinearLayout.VERTICAL);
        sheet = new ScrollView(game);
    }

    void attach() {
        game.addContentView(layer, new FrameLayout.LayoutParams(MATCH, MATCH));
        mouseTab = tab("鼠标", Gravity.START, 56, () -> toggle(true));
        modeTab = tab("", Gravity.CENTER_HORIZONTAL, 96, () -> {
            game.toggleTouchMode();
            updateModeTab();
        });
        updateModeTab();
        toolsTab = tab("工具", Gravity.END, 56, () -> toggle(false));
        resetTab = button("复位视图", game::resetViewport);
        resetTab.setVisibility(View.GONE);
        FrameLayout.LayoutParams resetPosition = new FrameLayout.LayoutParams(dp(96), dp(42),
                Gravity.BOTTOM | Gravity.END);
        resetPosition.setMargins(dp(12), 0, dp(12), dp(12));
        layer.addView(resetTab, resetPosition);
        sheet.setBackground(shape(0xeb172332, 16));
        sheet.setFillViewport(false);
        sheet.setVisibility(View.GONE);
        int pad = dp(7);
        sheet.setPadding(pad, pad, pad, pad);
        sheet.setClickable(true);
        sheet.addView(panel);
        layer.addView(sheet);
    }

    private void updateModeTab() {
        boolean trackpad = game.isTouchscreenTrackpad();
        modeTab.setText(trackpad ? "触控板模式" : "直接触控");
        modeTab.setContentDescription(trackpad ? "触控板模式，点按切换为直接触控" : "直接触控，点按切换为触控板模式");
    }

    void showViewReset(boolean zoomed) {
        resetTab.setVisibility(zoomed ? View.VISIBLE : View.GONE);
    }

    private TextView tab(String label, int edge, int width, Runnable action) {
        TextView v = button(label, action);
        v.setBackground(shape(0xbf192635, 24));
        FrameLayout.LayoutParams p = new FrameLayout.LayoutParams(dp(width), dp(42), Gravity.TOP | edge);
        p.setMargins(dp(12), dp(12), dp(12), 0);
        layer.addView(v, p);
        return v;
    }

    private void toggle(boolean mouse) {
        boolean close = sheet.getVisibility() == View.VISIBLE && mouseOpen == mouse;
        collapse();
        if (close) return;
        mouseOpen = mouse;
        panel.removeAllViews();
        if (mouse) mousePanel(); else keyPanel();
        int height = Math.min(dp(mouse ? 245 : 269), Math.max(dp(96), layer.getHeight() - dp(74)));
        FrameLayout.LayoutParams p = new FrameLayout.LayoutParams(Math.min(dp(mouse ? 240 : 282), layer.getWidth() - dp(24)),
                height, Gravity.TOP | (mouse ? Gravity.START : Gravity.END));
        p.setMargins(dp(12), dp(60), dp(12), dp(8));
        sheet.setLayoutParams(p);
        sheet.scrollTo(0, 0);
        sheet.setVisibility(View.VISIBLE);
        (mouse ? mouseTab : toolsTab).setText("收起");
    }

    boolean collapse() {
        if (sheet.getVisibility() != View.VISIBLE) return false;
        release();
        sheet.setVisibility(View.GONE);
        mouseTab.setText("鼠标");
        toolsTab.setText("工具");
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
        FlashLinkQuality active = game.currentQuality();
        if (active != null) {
            TextView quality = button("串流画质\n" + active.displayText(), () -> {
                collapse();
                new FlashLinkQualityDialog(game, active).show();
            });
            panel.addView(quality, new LinearLayout.LayoutParams(MATCH, dp(60)));
        }
        LinearLayout row = row();
        item(row, "空格", () -> key(KeyEvent.KEYCODE_SPACE));
        item(row, "回车", () -> key(KeyEvent.KEYCODE_ENTER));
        item(row, "Esc", () -> key(KeyEvent.KEYCODE_ESCAPE));
        row = row();
        item(row, "上一页", () -> key(KeyEvent.KEYCODE_PAGE_UP));
        item(row, "下一页", () -> key(KeyEvent.KEYCODE_PAGE_DOWN));
        row = row();
        item(row, "输入文字", this::textDialog);
        item(row, "系统键盘", () -> { collapse(); game.toggleKeyboard(); });
        item(row(), "断开连接", game::leaveStream);
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
        LinearLayout.LayoutParams p = new LinearLayout.LayoutParams(0, dp(42), 1f);
        p.setMargins(dp(2), dp(2), dp(2), dp(2));
        row.addView(view, p);
        return view;
    }

    private TextView button(String label, Runnable action) {
        TextView view = new TextView(game);
        view.setText(label);
        view.setGravity(Gravity.CENTER);
        view.setTextColor(Color.WHITE);
        view.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        view.setTextSize(14);
        view.setBackground(shape(0xff29394a, 9));
        view.setOnClickListener(v -> action.run());
        view.setFocusable(true);
        return view;
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
