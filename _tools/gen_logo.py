"""
生成 DzsSpeedy 斗战神游戏加速器 logo
- 主色: #0a0a0a (深黑底)
- 强调: #c9a96e (暗金) 文字 / #8b1a1a (血红) 装饰
- 风格: 暗黑国风
"""
from PIL import Image, ImageDraw, ImageFont, ImageFilter
import os

FONT_PATH = r"C:/Windows/Fonts/msyhbd.ttc"  # 微软雅黑 Bold
FONT_PATH_REG = r"C:/Windows/Fonts/msyh.ttc"

OUT = r"E:/projects/gamescript/OpenSpeedy/src-tauri/icons"

# ── 调色板 ──
BG_DARK = (10, 10, 10, 255)          # #0a0a0a 主背景
BG_PANEL = (24, 20, 18, 255)         # #181412 面板
GOLD = (201, 169, 110, 255)          # #c9a96e 暗金
GOLD_DARK = (140, 110, 60, 255)      # #8c6e3c
CRIMSON = (139, 26, 26, 255)         # #8b1a1a 血红
CRIMSON_BRIGHT = (180, 40, 40, 255)
TEXT_LIGHT = (230, 220, 200, 255)    # 米黄文字

def make_square_icon(size: int) -> Image.Image:
    """生成正方形深色图标: 居中放"斗"字暗金/血红色块, 边角有金线边框"""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    # 圆角矩形底
    pad = max(2, size // 32)
    d.rounded_rectangle([pad, pad, size - pad, size - pad],
                          radius=size // 8, fill=BG_DARK)
    # 金色内边框
    border_w = max(1, size // 64)
    inner = pad + border_w * 2
    d.rounded_rectangle([inner, inner, size - inner, size - inner],
                          radius=size // 10, outline=GOLD, width=border_w)
    # 中央"斗"字
    char_size = int(size * 0.55)
    font = ImageFont.truetype(FONT_PATH, char_size)
    text = "斗"
    bbox = d.textbbox((0, 0), text, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x = (size - tw) // 2 - bbox[0]
    y = (size - th) // 2 - bbox[1]
    # 阴影血红
    d.text((x + size // 64, y + size // 64), text, font=font, fill=CRIMSON)
    # 主暗金
    d.text((x, y), text, font=font, fill=GOLD)
    return img

def make_text_logo(width: int, height: int) -> Image.Image:
    """生成宽屏文字 logo: "斗战神游戏加速器" + 副标题"""
    img = Image.new("RGBA", (width, height), BG_DARK)
    d = ImageDraw.Draw(img)
    # 上方主标题"斗战神"
    main_size = int(height * 0.42)
    main_font = ImageFont.truetype(FONT_PATH, main_size)
    main_text = "斗战神"
    bbox = d.textbbox((0, 0), main_text, font=main_font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x = (width - tw) // 2 - bbox[0]
    y = int(height * 0.08) - bbox[1]
    # 暗金主
    d.text((x, y), main_text, font=main_font, fill=GOLD)
    # 下方副标题"游戏加速器"
    sub_size = int(height * 0.16)
    sub_font = ImageFont.truetype(FONT_PATH_REG, sub_size)
    sub_text = "游戏加速器"
    bbox2 = d.textbbox((0, 0), sub_text, font=sub_font)
    sw, sh = bbox2[2] - bbox2[0], bbox2[3] - bbox2[1]
    sx = (width - sw) // 2 - bbox2[0]
    sy = y + th + int(height * 0.06) - bbox2[1]
    d.text((sx, sy), sub_text, font=sub_font, fill=TEXT_LIGHT)
    # 装饰红线 (左右两条)
    line_w = max(2, height // 40)
    margin = int(width * 0.06)
    line_y = sy + sh + int(height * 0.05)
    d.rectangle([margin, line_y, width - margin, line_y + line_w], fill=CRIMSON)
    # 底部署名 (英文)
    en_size = int(height * 0.08)
    try:
        en_font = ImageFont.truetype(r"C:/Windows/Fonts/consola.ttf", en_size)
    except OSError:
        en_font = sub_font
    en_text = "DzsSpeedy"
    bbox3 = d.textbbox((0, 0), en_text, font=en_font)
    ew = bbox3[2] - bbox3[0]
    ex = (width - ew) // 2 - bbox3[0]
    ey = line_y + line_w + int(height * 0.04) - bbox3[1]
    d.text((ex, ey), en_text, font=en_font, fill=GOLD_DARK)
    return img

# 1) 方形图标 (所有 Tauri 平台都需要的)
for size, name in [(32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png"),
                    (256, "icon.png"), (107, "Square107x107Logo.png"),
                    (142, "Square142x142Logo.png"), (150, "Square150x150Logo.png"),
                    (284, "Square284x284Logo.png"), (30, "Square30x30Logo.png"),
                    (310, "Square310x310Logo.png"), (44, "Square44x44Logo.png"),
                    (71, "Square71x71Logo.png"), (89, "Square89x89Logo.png"),
                    (50, "StoreLogo.png")]:
    icon = make_square_icon(size)
    out = os.path.join(OUT, name)
    icon.save(out, "PNG")
    print(f"  {name:30s} {size}x{size}")

# 2) Windows ICO (multi-size)
ico_sizes = [16, 24, 32, 48, 64, 128, 256]
ico_imgs = [make_square_icon(s) for s in ico_sizes]
base = ico_imgs[-1]  # 256x256
ico_path = os.path.join(OUT, "icon.ico")
base.save(ico_path, format="ICO", sizes=[(s, s) for s in ico_sizes])
print(f"  icon.ico  multi-size: {ico_sizes}")

# 3) 文字 logo (用于 README / 关于面板)
logo = make_text_logo(800, 240)
logo.save(os.path.join(OUT, "logo-text.png"))
print(f"  logo-text.png  800x240")

print("Done.")
