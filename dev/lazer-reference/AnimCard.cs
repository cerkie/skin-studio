using System;
using osu.Framework.Graphics;
using osu.Framework.Graphics.Containers;
using osu.Framework.Graphics.Shapes;
using osu.Framework.Graphics.Sprites;
using osu.Framework.Graphics.Textures;
using osu.Framework.Input.Events;
using osu.Game.Graphics;
using osu.Game.Graphics.Sprites;
using osuTK;

namespace LazerPreview
{
    /// <summary>Something that can be dragged; reports how far the pointer moved.</summary>
    public partial class DragHandle : Container
    {
        public Action<Vector2> Moved;

        protected override bool OnHover(HoverEvent e) => true;
        protected override bool OnMouseDown(MouseDownEvent e) => true;
        protected override bool OnDragStart(DragStartEvent e) => true;

        protected override void OnDrag(DragEvent e) => Moved?.Invoke(e.ScreenSpaceMousePosition - e.ScreenSpaceLastMousePosition);
    }

    /// <summary>The right edge of a card, which resizes it: a thin line shows where it is when the pointer is over it.</summary>
    public partial class EdgeHandle : DragHandle, osu.Framework.Graphics.Cursor.IHasTooltip
    {
        public osu.Framework.Localisation.LocalisableString TooltipText => "Drag to resize";

        private readonly Box line;
        private const float rest = 0.35f;

        public EdgeHandle()
        {
            Width = 10;
            Anchor = Anchor.TopRight;
            Origin = Anchor.TopRight;
            Child = line = new Box { Width = 3, RelativeSizeAxes = Axes.Y, Height = 0.5f, Anchor = Anchor.CentreRight, Origin = Anchor.CentreRight, Colour = new Colour4(0x7b, 0x61, 0xe8, 255), Alpha = rest };
        }

        protected override bool OnHover(HoverEvent e)
        {
            line.Alpha = 0.9f;
            return true;
        }

        protected override void OnHoverLost(HoverLostEvent e) => line.Alpha = rest;
    }

    /// <summary>
    /// The animation being made, playing in a small card over the picture. It can be moved by its title, resized by its right
    /// edge and folded down to its title.
    /// </summary>
    public partial class AnimCard : Container
    {
        // one set of measures for the whole card
        private const float pad = 12, gap = 10, headerH = 44, rowH = 36, footerH = 12;
        private const float playW = 48, speedW = 60;

        private readonly OsuSpriteText counter;
        private readonly SpriteIcon chevron, playIcon;
        private readonly Container canvas, body;
        private readonly Sprite frame;
        private readonly BarSlider scrub;
        private readonly OsuSpriteText speedText;
        private readonly BarButton play, speedChip;
        private readonly EdgeHandle grip;

        private float width = 320;
        private bool collapsed;
        private bool placed;
        private bool playing = true;
        private double pos;
        private double speed = 1;
        private int mw = 1, mh = 1;
        private Texture[] seen;

        private static readonly double[] speeds = { 0.25, 0.5, 1, 2 };

        public AnimCard()
        {
            Masking = true;
            CornerRadius = 12;
            BorderThickness = 1;
            BorderColour = new Colour4(255, 255, 255, 40);
            Anchor = Anchor.TopLeft;
            Origin = Anchor.TopLeft;

            var dark = new Colour4(0x2a, 0x27, 0x33, 255);
            var darkHover = new Colour4(0x3a, 0x36, 0x47, 255);

            DragHandle top;
            BarButton fold, smaller, larger;

            InternalChildren = new Drawable[]
            {
                new Box { RelativeSizeAxes = Axes.Both, Colour = new Colour4(0x1c, 0x1a, 0x22, 240) },
                top = new DragHandle
                {
                    RelativeSizeAxes = Axes.X,
                    Height = headerH,
                    Children = new Drawable[]
                    {
                        new OsuSpriteText { Font = OsuFont.GetFont(size: 20, weight: FontWeight.SemiBold), Text = "Animation", X = pad, Anchor = Anchor.CentreLeft, Origin = Anchor.CentreLeft, Colour = new Colour4(0xe6, 0xe3, 0xf0, 255) },
                        fold = new BarButton(Colour4.Transparent, dark, new Vector2(36, 32))
                        {
                            Anchor = Anchor.CentreRight,
                            Origin = Anchor.CentreRight,
                            X = -(pad - 4),
                            TooltipText = "Fold away or unfold",
                            Child = chevron = new SpriteIcon { Icon = FontAwesome.Solid.ChevronUp, Size = new Vector2(14), Anchor = Anchor.Centre, Origin = Anchor.Centre },
                        },
                        larger = new BarButton(Colour4.Transparent, dark, new Vector2(36, 32))
                        {
                            Anchor = Anchor.CentreRight,
                            Origin = Anchor.CentreRight,
                            X = -(pad - 4 + 40),
                            TooltipText = "Make the card larger",
                            Child = new SpriteIcon { Icon = FontAwesome.Solid.Plus, Size = new Vector2(14), Anchor = Anchor.Centre, Origin = Anchor.Centre },
                        },
                        smaller = new BarButton(Colour4.Transparent, dark, new Vector2(36, 32))
                        {
                            Anchor = Anchor.CentreRight,
                            Origin = Anchor.CentreRight,
                            X = -(pad - 4 + 80),
                            TooltipText = "Make the card smaller",
                            Child = new SpriteIcon { Icon = FontAwesome.Solid.Minus, Size = new Vector2(14), Anchor = Anchor.Centre, Origin = Anchor.Centre },
                        },
                        counter = new OsuSpriteText { Font = OsuFont.GetFont(size: 15), X = -(pad + 80 + 40), Anchor = Anchor.CentreRight, Origin = Anchor.CentreRight, Colour = new Colour4(0x9a, 0x96, 0xae, 255) },
                    },
                },
                body = new Container
                {
                    RelativeSizeAxes = Axes.Both,
                    Children = new Drawable[]
                    {
                        canvas = new Container
                        {
                            X = pad,
                            Y = headerH,
                            Masking = true,
                            CornerRadius = 4,
                            BorderThickness = 2,
                            BorderColour = new Colour4(0x50, 0x47, 0x6b, 255),
                            Children = new Drawable[]
                            {
                                new Box { RelativeSizeAxes = Axes.Both, Colour = new Colour4(0x18, 0x17, 0x1c, 255) },
                                frame = new Sprite { Anchor = Anchor.Centre, Origin = Anchor.Centre },
                            },
                        },
                        play = new BarButton(new Colour4(0x62, 0x45, 0xd9, 255), new Colour4(0x75, 0x5a, 0xe8, 255), new Vector2(playW, rowH))
                        {
                            X = pad,
                            TooltipText = "Play / pause the animation",
                            Child = playIcon = new SpriteIcon { Icon = FontAwesome.Solid.Pause, Size = new Vector2(16), Anchor = Anchor.Centre, Origin = Anchor.Centre },
                        },
                        scrub = new BarSlider(1.2f) { TooltipText = "Scrub through the animation" },
                        speedChip = new BarButton(dark, darkHover, new Vector2(speedW, rowH))
                        {
                            TooltipText = "Animation speed (click to change)",
                            Child = speedText = new OsuSpriteText { Font = OsuFont.GetFont(size: 17), Anchor = Anchor.Centre, Origin = Anchor.Centre, Text = "1x" },
                        },
                    },
                },
                grip = new EdgeHandle(),
            };

            top.Moved = d => Position += d;
            grip.Moved = d => width = Math.Clamp(width + d.X, 260, 900);
            fold.Clicked = () => collapsed = !collapsed;
            larger.Clicked = () => width = Math.Clamp(width + 60, 260, 900);
            smaller.Clicked = () => width = Math.Clamp(width - 60, 260, 900);
            play.Clicked = () => playing = !playing;

            speedChip.Clicked = () =>
            {
                double next = speeds[0];

                foreach (double s in speeds)
                {
                    if (s > speed + 0.001)
                    {
                        next = s;
                        break;
                    }
                }

                speed = next;
            };
        }

        // updated even while it is hidden, so that it can show itself when there is an animation
        public override bool IsPresent => true;

        protected override bool OnClick(ClickEvent e) => true;
        protected override bool OnMouseDown(MouseDownEvent e) => true;
        protected override bool OnHover(HoverEvent e) => true;

        protected override void Update()
        {
            base.Update();

            var frames = FrameSkin.Frames;
            int n = frames.Length;

            Alpha = n > 0 ? 1 : 0;

            if (n == 0)
                return;

            if (!ReferenceEquals(frames, seen))
            {
                seen = frames;
                mw = mh = 1;

                foreach (var t in frames)
                {
                    mw = Math.Max(mw, t.Width);
                    mh = Math.Max(mh, t.Height);
                }
            }

            float inner = width - pad * 2;
            float scale = inner / mw;
            float imageH = Math.Max(mh * scale, 24);
            float rowY = headerH + imageH + gap;

            Width = width;
            Height = collapsed ? headerH : rowY + rowH + footerH;
            body.Alpha = collapsed ? 0 : 1;
            grip.Alpha = collapsed ? 0 : 1;
            grip.Y = headerH;
            grip.Height = Math.Max(Height - headerH, 0);
            chevron.Icon = collapsed ? FontAwesome.Solid.ChevronDown : FontAwesome.Solid.ChevronUp;

            canvas.Size = new Vector2(inner, imageH);
            play.Y = rowY;
            scrub.Position = new Vector2(pad + playW + gap, rowY + (rowH - scrub.Height) / 2);
            scrub.Width = Math.Max(inner - playW - speedW - gap * 2, 20);
            speedChip.Position = new Vector2(width - pad - speedW, rowY);

            double loop = n * FrameSkin.FrameLength;

            if (scrub.Dragging)
                pos = scrub.Value * loop;
            else if (playing)
                pos += Time.Elapsed * speed;

            pos = ((pos % loop) + loop) % loop;
            int idx = Math.Min((int)(pos / FrameSkin.FrameLength), n - 1);

            if (!scrub.Dragging)
                scrub.Value = pos / loop;

            var tex = frames[idx];
            frame.Texture = tex;
            frame.Size = new Vector2(tex.Width, tex.Height) * scale;

            playIcon.Icon = playing ? FontAwesome.Solid.Pause : FontAwesome.Solid.Play;
            speedText.Text = (Math.Round(speed * 100) / 100).ToString("0.##") + "x";
            counter.Alpha = width >= 440 ? 1 : 0;
            counter.Text = (idx + 1) + " / " + n + "  ·  " + Math.Round(1000 / FrameSkin.FrameLength) + " fps";

            if (Parent != null && Parent.DrawWidth > 0)
            {
                if (!placed)
                {
                    placed = true;
                    Position = new Vector2(Parent.DrawWidth - width - 16, 16);
                }

                X = Math.Clamp(X, 0, Math.Max(Parent.DrawWidth - Width, 0));
                Y = Math.Clamp(Y, 0, Math.Max(Parent.DrawHeight - Height, 0));
            }
        }
    }
}
