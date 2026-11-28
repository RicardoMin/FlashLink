// SPDX-License-Identifier: GPL-3.0-only
package com.limelight;

import android.app.Activity;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.view.DisplayCutout;
import android.view.Gravity;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;

/** Read-only badge. Samples arrive from the decoder once per completed statistics window. */
final class FlashLinkStreamStatus extends TextView {
    private volatile boolean sampling;
    private int fps = -1;
    private boolean poorConnection;
    // The decoder stops publishing windows when no more frames arrive.
    private final Runnable stale = () -> showRate(0);

    FlashLinkStreamStatus(Activity activity) {
        super(activity);
        setVisibility(GONE);
        setTextSize(12);
        setTextColor(0xfff0f5f5);
        setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        setGravity(Gravity.CENTER);
        setPadding(dp(12), dp(4), dp(12), dp(4));
        GradientDrawable pill = new GradientDrawable();
        pill.setColor(0xcd19242a);
        pill.setCornerRadius(dp(20));
        setBackground(pill);
        setClickable(false);
        setFocusable(false);
        FrameLayout.LayoutParams position = new FrameLayout.LayoutParams(-2, -2, Gravity.TOP | Gravity.CENTER_HORIZONTAL);
        position.topMargin = dp(8);
        activity.addContentView(this, position);
        setOnApplyWindowInsetsListener((v, insets) -> {
            int top = insets.getSystemWindowInsetTop();
            if (Build.VERSION.SDK_INT >= 28) {
                DisplayCutout cutout = insets.getDisplayCutout();
                if (cutout != null) top = Math.max(top, cutout.getSafeInsetTop());
            }
            int margin = top + dp(8);
            FrameLayout.LayoutParams params = (FrameLayout.LayoutParams) getLayoutParams();
            if (params.topMargin != margin) {
                params.topMargin = margin;
                setLayoutParams(params);
            }
            return insets;
        });
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    void setShown(boolean shown) {
        if (sampling == shown) return;
        sampling = shown;
        removeCallbacks(stale);
        setVisibility(shown ? View.VISIBLE : View.GONE);
        if (shown) {
            showRate(-1);
            requestApplyInsets();
            postDelayed(stale, 2500);
        }
    }

    void onSample(int renderedFps) {
        if (!sampling) return;
        post(() -> {
            if (!sampling) return;
            showRate(renderedFps);
            removeCallbacks(stale);
            postDelayed(stale, 2500);
        });
    }

    void setPoorConnection(boolean poor) {
        poorConnection = poor;
        if (sampling) showRate(fps);
    }

    private void showRate(int rate) {
        fps = rate;
        setText((poorConnection ? "网络不稳" : "已连接") + "  ·  " + (rate < 0 ? "—" : rate + "帧"));
    }
}
