// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.content.Context;
import android.content.SharedPreferences;
import android.content.res.ColorStateList;
import android.graphics.Rect;
import android.graphics.drawable.GradientDrawable;
import android.graphics.drawable.InsetDrawable;
import android.graphics.drawable.RippleDrawable;
import android.os.Build;
import android.view.DisplayCutout;
import android.view.Gravity;
import android.view.MotionEvent;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.WindowInsets;
import android.widget.FrameLayout;
import android.widget.ImageButton;
import android.widget.ScrollView;

/** A dock in the player's window. When closed, only the orb participates in hit testing. */
final class FlashLinkControlDock extends FrameLayout {
    private final SharedPreferences preferences;
    private final View outside;
    private final ImageButton orb;
    private final ScrollView sheet;
    private final Rect safe = new Rect();
    private final int[] location = new int[2];
    private final int[] rootLocation = new int[2];
    private final int slop;
    private boolean onRight;
    private float heightFraction;
    private int pointer = -1;
    private float downX, downY;
    private int initialLeft, initialTop;
    private boolean dragging;
    private final Runnable fade;

    FlashLinkControlDock(Context context, ScrollView sheet, Runnable toggle, Runnable dismiss) {
        super(context);
        this.sheet = sheet;
        slop = ViewConfiguration.get(context).getScaledTouchSlop();
        preferences = context.getSharedPreferences("flashlink_control_dock", Context.MODE_PRIVATE);
        onRight = preferences.getBoolean("right", true);
        heightFraction = preferences.getFloat("height_fraction", 0.35f);

        outside = new View(context);
        outside.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
        outside.setOnClickListener(v -> dismiss.run());
        outside.setVisibility(GONE);
        addView(outside, new LayoutParams(-1, -1));
        sheet.setVisibility(GONE);
        addView(sheet);

        orb = new ImageButton(context);
        orb.setContentDescription("打开控制中心");
        orb.setScaleType(ImageButton.ScaleType.CENTER);
        orb.setPadding(0, 0, 0, 0);
        orb.setImageResource(R.drawable.fl_bolt);
        GradientDrawable disc = new GradientDrawable();
        disc.setShape(GradientDrawable.OVAL);
        disc.setColor(0xff25354c);
        disc.setStroke(dp(1), 0xffa9cfff);
        RippleDrawable ripple = new RippleDrawable(ColorStateList.valueOf(0x557ab3ff), disc, null);
        orb.setBackground(new InsetDrawable(ripple, dp(7)));
        orb.setOnClickListener(v -> toggle.run());
        orb.setOnTouchListener((v, event) -> touchOrb(event));
        addView(orb, new LayoutParams(dp(48), dp(48), Gravity.TOP | Gravity.LEFT));
        fade = () -> orb.animate().alpha(0.40f).setDuration(240).start();
        scheduleFade();
    }

    void setPanelOpen(boolean open) {
        outside.setVisibility(open ? VISIBLE : GONE);
        sheet.setVisibility(open ? VISIBLE : GONE);
        orb.setContentDescription(open ? "收起控制中心" : "打开控制中心");
        wakeOrb();
        if (!open) scheduleFade();
    }

    private void wakeOrb() {
        orb.removeCallbacks(fade);
        orb.animate().cancel();
        orb.setAlpha(1f);
    }

    private void scheduleFade() {
        orb.removeCallbacks(fade);
        if (sheet.getVisibility() != VISIBLE) orb.postDelayed(fade, 1500);
    }

    @Override protected void onDetachedFromWindow() {
        orb.removeCallbacks(fade);
        orb.animate().cancel();
        super.onDetachedFromWindow();
    }

    @Override public WindowInsets onApplyWindowInsets(WindowInsets insets) {
        requestLayout();
        return super.onApplyWindowInsets(insets);
    }

    // Express window insets in layer coordinates. Some devices already inset the content view.
    private void updateSafeArea(int width, int height) {
        safe.set(0, 0, width, height);
        WindowInsets insets = getRootWindowInsets();
        if (insets != null) {
            int l = insets.getSystemWindowInsetLeft(), r = insets.getSystemWindowInsetRight();
            int t = insets.getSystemWindowInsetTop(), b = insets.getSystemWindowInsetBottom();
            if (Build.VERSION.SDK_INT >= 28) {
                DisplayCutout cutout = insets.getDisplayCutout();
                if (cutout != null) {
                    l = Math.max(l, cutout.getSafeInsetLeft());
                    r = Math.max(r, cutout.getSafeInsetRight());
                    t = Math.max(t, cutout.getSafeInsetTop());
                    b = Math.max(b, cutout.getSafeInsetBottom());
                }
            }
            View root = getRootView();
            root.getLocationInWindow(rootLocation);
            getLocationInWindow(location);
            int x = location[0] - rootLocation[0], y = location[1] - rootLocation[1];
            if (root.getWidth() > 0 && root.getHeight() > 0) {
                safe.intersect(l - x, t - y, root.getWidth() - r - x, root.getHeight() - b - y);
            }
        }
        if (safe.width() > dp(60) && safe.height() > dp(60)) safe.inset(dp(4), dp(4));
    }

    @Override protected void onMeasure(int widthSpec, int heightSpec) {
        super.onMeasure(widthSpec, heightSpec);
        if (sheet.getVisibility() != VISIBLE) return;
        updateSafeArea(getMeasuredWidth(), getMeasuredHeight());
        int width = Math.max(1, Math.min(dp(302), safe.width() - orb.getMeasuredWidth() - dp(4)));
        sheet.measure(MeasureSpec.makeMeasureSpec(width, MeasureSpec.EXACTLY),
                MeasureSpec.makeMeasureSpec(safe.height(), MeasureSpec.AT_MOST));
    }

    @Override protected void onLayout(boolean changed, int l, int t, int r, int b) {
        // Keep the orb under the finger if a layout pass happens during a drag.
        int oldLeft = orb.getLeft(), oldTop = orb.getTop();
        super.onLayout(changed, l, t, r, b);
        updateSafeArea(r - l, b - t);
        int right = safe.right - orb.getMeasuredWidth(), bottom = safe.bottom - orb.getMeasuredHeight();
        int dockTop = safe.top + Math.round(Math.max(0, bottom - safe.top) * heightFraction);
        placeOrb(dragging ? oldLeft : (onRight ? right : safe.left), dragging ? oldTop : dockTop);
        if (sheet.getVisibility() == VISIBLE) {
            int w = sheet.getMeasuredWidth(), h = sheet.getMeasuredHeight();
            int x = onRight ? orb.getLeft() - dp(4) - w : orb.getRight() + dp(4);
            int y = clamp(orb.getTop() + (orb.getHeight() - h) / 2, safe.top, safe.bottom - h);
            sheet.layout(x, y, x + w, y + h);
        }
    }

    private void placeOrb(int x, int y) {
        int w = orb.getMeasuredWidth(), h = orb.getMeasuredHeight();
        x = clamp(x, safe.left, safe.right - w);
        y = clamp(y, safe.top, safe.bottom - h);
        orb.layout(x, y, x + w, y + h);
    }

    private boolean touchOrb(MotionEvent event) {
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                pointer = event.getPointerId(0);
                downX = event.getRawX();
                downY = event.getRawY();
                initialLeft = orb.getLeft();
                initialTop = orb.getTop();
                wakeOrb();
                orb.setPressed(true);
                break;
            case MotionEvent.ACTION_MOVE:
                if (pointer < 0 || event.getPointerId(0) != pointer) break;
                float dx = event.getRawX() - downX, dy = event.getRawY() - downY;
                if (!dragging && Math.hypot(dx, dy) > slop) {
                    dragging = true;
                    // Moving an open dock also dismisses its menu and releases held buttons.
                    if (sheet.getVisibility() == VISIBLE) outside.performClick();
                    wakeOrb();
                    orb.setPressed(false);
                }
                if (dragging) placeOrb(initialLeft + Math.round(dx), initialTop + Math.round(dy));
                break;
            case MotionEvent.ACTION_POINTER_UP:
            case MotionEvent.ACTION_UP:
                if (event.getPointerId(event.getActionIndex()) != pointer) break;
                if (dragging) {
                    onRight = orb.getLeft() + orb.getWidth() / 2 >= safe.centerX();
                    int range = safe.height() - orb.getHeight();
                    if (range > 0) heightFraction = (orb.getTop() - safe.top) / (float)range;
                    preferences.edit().putBoolean("right", onRight).putFloat("height_fraction", heightFraction).apply();
                } else {
                    orb.performClick();
                }
                // fall through to clear the gesture and snap to the saved edge
            case MotionEvent.ACTION_CANCEL:
                pointer = -1;
                dragging = false;
                orb.setPressed(false);
                requestLayout();
                scheduleFade();
                break;
            default:
                break;
        }
        return true;
    }

    private static int clamp(int value, int min, int max) {
        return Math.max(min, Math.min(value, max));
    }

    private int dp(int value) {
        return Math.round(getResources().getDisplayMetrics().density * value);
    }
}
