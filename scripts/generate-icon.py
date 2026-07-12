#!/usr/bin/env python3
"""生成 maclean 的 macOS 应用图标 (.icns)"""

import os
import subprocess
from PIL import Image, ImageDraw

SIZE = 1024
RADIUS = int(SIZE * 0.22)  # 圆角半径

# 蓝绿色渐变背景 (从上到下)
TOP = (82, 183, 222)     # 浅蓝
BOTTOM = (76, 175, 80)   # 绿色

OUTPUT_DIR = "assets/icon"
ICNS_NAME = "AppIcon.icns"
ICONSET_DIR = os.path.join(OUTPUT_DIR, "AppIcon.iconset")


def rounded_rect(draw, xy, radius, fill):
    """绘制圆角矩形"""
    x0, y0, x1, y1 = xy
    draw.rounded_rectangle(xy, radius=radius, fill=fill)


def draw_broom(draw):
    """绘制倾斜的简约扫帚"""
    import math

    # 扫帚整体向右倾斜 25 度
    angle = math.radians(25)
    cos_a = math.cos(angle)
    sin_a = math.sin(angle)

    def rotate(px, py, cx, cy):
        x = px - cx
        y = py - cy
        return (cx + x * cos_a - y * sin_a, cy + x * sin_a + y * cos_a)

    cx, cy = SIZE // 2, int(SIZE * 0.55)

    # 扫帚柄参数 (未旋转坐标系)
    stick_color = (139, 90, 43)
    stick_top = (cx, int(SIZE * 0.12))
    stick_bottom = (cx, int(SIZE * 0.50))
    stick_width = int(SIZE * 0.045)

    # 扫帚柄阴影
    shadow_offset = int(SIZE * 0.012)
    stick_points_shadow = [
        rotate(stick_top[0] - stick_width // 2 + shadow_offset, stick_top[1] + shadow_offset, cx, cy),
        rotate(stick_top[0] + stick_width // 2 + shadow_offset, stick_top[1] + shadow_offset, cx, cy),
        rotate(stick_bottom[0] + stick_width // 2 + shadow_offset, stick_bottom[1] + shadow_offset, cx, cy),
        rotate(stick_bottom[0] - stick_width // 2 + shadow_offset, stick_bottom[1] + shadow_offset, cx, cy),
    ]
    draw.polygon(stick_points_shadow, fill=(0, 0, 0, 40))

    # 扫帚柄主体
    stick_points = [
        rotate(stick_top[0] - stick_width // 2, stick_top[1], cx, cy),
        rotate(stick_top[0] + stick_width // 2, stick_top[1], cx, cy),
        rotate(stick_bottom[0] + stick_width // 2, stick_bottom[1], cx, cy),
        rotate(stick_bottom[0] - stick_width // 2, stick_bottom[1], cx, cy),
    ]
    draw.polygon(stick_points, fill=stick_color)

    # 扫帚头 (梯形)
    broom_color = (245, 222, 179)
    broom_shadow = (215, 190, 150)
    broom_top_y = int(SIZE * 0.48)
    broom_bottom_y = int(SIZE * 0.72)
    broom_top_w = int(SIZE * 0.18)
    broom_bottom_w = int(SIZE * 0.34)

    # 扫帚头阴影
    head_points_shadow = [
        rotate(cx - broom_top_w // 2 + shadow_offset, broom_top_y + shadow_offset, cx, cy),
        rotate(cx + broom_top_w // 2 + shadow_offset, broom_top_y + shadow_offset, cx, cy),
        rotate(cx + broom_bottom_w // 2 + shadow_offset, broom_bottom_y + shadow_offset, cx, cy),
        rotate(cx - broom_bottom_w // 2 + shadow_offset, broom_bottom_y + shadow_offset, cx, cy),
    ]
    draw.polygon(head_points_shadow, fill=(0, 0, 0, 40))

    # 扫帚头主体
    head_points = [
        rotate(cx - broom_top_w // 2, broom_top_y, cx, cy),
        rotate(cx + broom_top_w // 2, broom_top_y, cx, cy),
        rotate(cx + broom_bottom_w // 2, broom_bottom_y, cx, cy),
        rotate(cx - broom_bottom_w // 2, broom_bottom_y, cx, cy),
    ]
    draw.polygon(head_points, fill=broom_color)

    # 扫帚头顶部束带
    band_color = (160, 120, 80)
    band_points = [
        rotate(cx - broom_top_w // 2 - int(SIZE * 0.01), broom_top_y - int(SIZE * 0.015), cx, cy),
        rotate(cx + broom_top_w // 2 + int(SIZE * 0.01), broom_top_y - int(SIZE * 0.015), cx, cy),
        rotate(cx + broom_top_w // 2 + int(SIZE * 0.01), broom_top_y + int(SIZE * 0.04), cx, cy),
        rotate(cx - broom_top_w // 2 - int(SIZE * 0.01), broom_top_y + int(SIZE * 0.04), cx, cy),
    ]
    draw.polygon(band_points, fill=band_color)

    # 扫帚鬃毛
    hair_color = (210, 180, 140)
    hair_bottom_y = int(SIZE * 0.82)
    hair_count = 9
    hair_x_step = broom_bottom_w / (hair_count - 1)
    for i in range(hair_count):
        x = cx - broom_bottom_w // 2 + int(i * hair_x_step)
        # 鬃毛稍微向外散开
        spread = (i - hair_count // 2) * int(SIZE * 0.012)
        top = rotate(x, broom_bottom_y - int(SIZE * 0.01), cx, cy)
        bottom = rotate(x + spread, hair_bottom_y, cx, cy)
        draw.line([top, bottom], fill=hair_color, width=int(SIZE * 0.022))

    # 闪光/星星 (清扫效果)
    star_color = (255, 255, 255)
    star_positions = [
        (int(SIZE * 0.78), int(SIZE * 0.32)),
        (int(SIZE * 0.70), int(SIZE * 0.24)),
        (int(SIZE * 0.84), int(SIZE * 0.42)),
    ]
    for sx, sy in star_positions:
        draw_star(draw, sx, sy, int(SIZE * 0.04), star_color)

    # 小灰尘点
    dust_color = (255, 255, 255)
    dust_positions = [
        (int(SIZE * 0.80), int(SIZE * 0.55)),
        (int(SIZE * 0.74), int(SIZE * 0.62)),
        (int(SIZE * 0.87), int(SIZE * 0.60)),
    ]
    for dx, dy in dust_positions:
        r = int(SIZE * 0.02)
        draw.ellipse([dx - r, dy - r, dx + r, dy + r], fill=dust_color)


def draw_star(draw, cx, cy, r, color):
    """绘制四角星"""
    points = []
    for i in range(8):
        angle = i * 3.14159 / 4
        radius = r if i % 2 == 0 else r * 0.4
        x = cx + radius * (1 if i in [0, 4] else (0 if i in [2, 6] else (1 if i == 1 else (-1 if i == 3 else (-1 if i == 5 else 1)))))
        # 简化为手动点
    # 更简单的十字星光
    draw.polygon(
        [(cx, cy - r), (cx + r * 0.25, cy - r * 0.25), (cx + r, cy), (cx + r * 0.25, cy + r * 0.25),
         (cx, cy + r), (cx - r * 0.25, cy + r * 0.25), (cx - r, cy), (cx - r * 0.25, cy - r * 0.25)],
        fill=color,
    )


def make_gradient_bg():
    """创建渐变背景"""
    img = Image.new("RGB", (SIZE, SIZE))
    for y in range(SIZE):
        ratio = y / (SIZE - 1)
        r = int(TOP[0] * (1 - ratio) + BOTTOM[0] * ratio)
        g = int(TOP[1] * (1 - ratio) + BOTTOM[1] * ratio)
        b = int(TOP[2] * (1 - ratio) + BOTTOM[2] * ratio)
        for x in range(SIZE):
            img.putpixel((x, y), (r, g, b))
    return img


def add_rounded_mask(img):
    """给图像添加圆角遮罩"""
    mask = Image.new("L", (SIZE, SIZE), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle([0, 0, SIZE, SIZE], radius=RADIUS, fill=255)

    # 添加轻微内阴影/描边效果
    border = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    border_draw = ImageDraw.Draw(border)
    border_draw.rounded_rectangle(
        [0, 0, SIZE - 1, SIZE - 1], radius=RADIUS, outline=(255, 255, 255, 40), width=2
    )

    img_rgba = img.convert("RGBA")
    img_rgba.putalpha(mask)
    return Image.alpha_composite(img_rgba, border)


def main():
    os.makedirs(OUTPUT_DIR, exist_ok=True)

    # 1. 生成渐变背景
    img = make_gradient_bg()
    draw = ImageDraw.Draw(img)

    # 2. 绘制扫帚
    draw_broom(draw)

    # 3. 应用圆角遮罩
    final = add_rounded_mask(img)

    # 4. 保存 1024 PNG
    png_path = os.path.join(OUTPUT_DIR, "maclean_icon_1024.png")
    final.save(png_path, "PNG")
    print(f"Saved: {png_path}")

    # 5. 生成 iconset
    if os.path.exists(ICONSET_DIR):
        subprocess.run(["rm", "-rf", ICONSET_DIR], check=False)
    os.makedirs(ICONSET_DIR)

    sizes = [16, 32, 64, 128, 256, 512, 1024]
    for s in sizes:
        resized = final.resize((s, s), Image.Resampling.LANCZOS)
        resized.save(os.path.join(ICONSET_DIR, f"icon_{s}x{s}.png"), "PNG")
        if s <= 512:
            # macOS 需要 @2x 版本
            resized2x = final.resize((s * 2, s * 2), Image.Resampling.LANCZOS)
            resized2x.save(os.path.join(ICONSET_DIR, f"icon_{s}x{s}@2x.png"), "PNG")

    # 6. 生成 .icns
    icns_path = os.path.join(OUTPUT_DIR, ICNS_NAME)
    subprocess.run(["iconutil", "-c", "icns", "-o", icns_path, ICONSET_DIR], check=True)
    subprocess.run(["rm", "-rf", ICONSET_DIR], check=False)
    print(f"Saved: {icns_path}")


if __name__ == "__main__":
    main()
