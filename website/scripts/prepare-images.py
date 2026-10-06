"""Optimize real captures and derive the social preview (requires Pillow).

Run from website/: python scripts/prepare-images.py
Original PNG captures remain available through the full-size links.
"""

from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

root = Path(__file__).resolve().parents[1] / "public"
for name in ("overview", "large-icons", "image-viewer", "properties"):
    with Image.open(root / "images" / f"{name}.png") as capture:
        capture.convert("RGB").save(root / "images" / f"{name}.webp", quality=95, method=6)
        print(f"{name}: {capture.width} x {capture.height}")

card = Image.new("RGB", (1200, 630), "#f2f1ee")
draw = ImageDraw.Draw(card)
font_candidates = (
    Path("C:/Windows/Fonts/segoeuib.ttf"),
    Path("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
)
font_path = next((path for path in font_candidates if path.exists()), None)
font = ImageFont.truetype(str(font_path), 42) if font_path else ImageFont.load_default(size=42)
small = ImageFont.truetype(str(font_path), 22) if font_path else ImageFont.load_default(size=22)
with Image.open(root / "icon.png") as icon:
    icon = icon.convert("RGBA").resize((88, 88), Image.Resampling.LANCZOS)
    card.paste(icon, (40, 32), icon)
draw.text((152, 24), "Windows File Explorer.", fill="#101014", font=font)
draw.text((154, 85), "On macOS, Linux and Windows.", fill="#2467d1", font=small)
with Image.open(root / "images" / "overview.png") as overview:
    overview = overview.convert("RGB")
    overview = overview.resize((1120, round(overview.height * 1120 / overview.width)), Image.Resampling.LANCZOS)
    card.paste(overview.crop((0, 0, 1120, 465)), (40, 145))
card.save(root / "images" / "social.png", optimize=True)
