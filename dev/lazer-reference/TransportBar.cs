using System;
using osu.Framework.Graphics;
using osu.Framework.Graphics.Containers;
using osu.Framework.Graphics.Cursor;
using osu.Framework.Graphics.Shapes;
using osu.Framework.Graphics.Sprites;
using osu.Framework.Input.Events;
using osu.Framework.Localisation;
using osu.Game.Graphics;
using osu.Game.Graphics.Sprites;
using osuTK;

namespace LazerPreview
{
    /// <summary>A button with a tooltip.</summary>
    public partial class BarButton : Container, IHasTooltip
    {
        public LocalisableString TooltipText { get; set; }
        public Action Clicked;

        private readonly Box background;
        private readonly Colour4 normal, hover;

        public BarButton(Colour4 normal, Colour4 hover, Vector2 size)
        {
            this.normal = normal;
            this.hover = hover;
            Size = size;
            Masking = true;
            CornerRadius = 5;
            InternalChildren = new Drawable[]
            {
                background = new Box { RelativeSizeAxes = Axes.Both, Colour = normal },
                content = new Container { RelativeSizeAxes = Axes.Both },
            };
        }

        private readonly Container content;

        // what is put in the button goes over its background
        protected override Container<Drawable> Content => content;

        protected override bool OnHover(HoverEvent e)
        {
            background.Colour = hover;
            return true;
        }

        protected override void OnHoverLost(HoverLostEvent e) => background.Colour = normal;

        protected override bool OnClick(ClickEvent e)
        {
            Clicked?.Invoke();
            return true;
        }
    }

    /// <summary>A horizontal slider (0 to 1) drawn as a thin line with a handle.</summary>
    public partial class BarSlider : Container, IHasTooltip
    {
        public LocalisableString TooltipText { get; set; }
        public Action<double> Changed;

        private double current;
        private readonly Box fill, handle;

        public bool Dragging { get; private set; }

        public double Value
        {
            get => current;
            set => current = Math.Clamp(value, 0, 1);
        }

        public BarSlider(float scale = 1)
        {
            Height = 30 * scale;

            Children = new Drawable[]
            {
                new Container
                {
                    RelativeSizeAxes = Axes.X,
                    Height = 6 * scale,
                    Anchor = Anchor.CentreLeft,
                    Origin = Anchor.CentreLeft,
                    Masking = true,
                    CornerRadius = 3 * scale,
                    Children = new Drawable[]
                    {
                        new Box { RelativeSizeAxes = Axes.Both, Colour = new Colour4(0x25, 0x23, 0x2c, 255) },
                        fill = new Box { RelativeSizeAxes = Axes.Both, Width = 0, Colour = new Colour4(0x7b, 0x61, 0xe8, 255) },
                    },
                },
                handle = new Box
                {
                    Size = new Vector2(8, 20) * scale,
                    Anchor = Anchor.CentreLeft,
                    Origin = Anchor.Centre,
                    Colour = new Colour4(0xb9, 0xb2, 0xd9, 255),
                },
            };
        }

        protected override void Update()
        {
            base.Update();
            fill.Width = (float)current;
            handle.X = handle.Width / 2 + (DrawWidth - handle.Width) * (float)current;
        }

        private void set(UIEvent e)
        {
            double f = Math.Clamp((ToLocalSpace(e.ScreenSpaceMousePosition).X - handle.Width / 2) / Math.Max(DrawWidth - handle.Width, 1), 0, 1);
            current = f;
            Changed?.Invoke(f);
        }

        protected override bool OnMouseDown(MouseDownEvent e)
        {
            if (e.Button != osuTK.Input.MouseButton.Left)
                return false;

            set(e);
            return true;
        }

        protected override bool OnDragStart(DragStartEvent e)
        {
            Dragging = true;
            return true;
        }

        protected override void OnDrag(DragEvent e) => set(e);

        protected override void OnDragEnd(DragEndEvent e) => Dragging = false;
    }

    /// <summary>The bar along the bottom of the picture: play, a new map, speed, seeking, volume and the time.</summary>
    public partial class TransportBar : Container
    {
        public Action PlayPause, NewMap;
        public Action<double> Seek, SetVolume, SetSpeed;

        private static readonly double[] speeds = { 0.5, 0.75, 1, 1.25, 1.5, 2 };

        private readonly SpriteIcon playIcon, volumeIcon;
        private readonly OsuSpriteText speedText, timeText;
        private readonly BarSlider seek, volume;
        private readonly Container left, right;

        // one set of measures for everything on the bar
        private const float edge = 16, gap = 12, groupGap = 24;
        private const float buttonW = 66, buttonH = 51, speedW = 96, volumeW = 150, timeW = 176;
        private const float leftWidth = (buttonW + gap) * 2 + speedW;
        private const float rightWidth = buttonW + gap + volumeW + gap + timeW;

        private double speed = 1;
        private double lastVolume = 0.5;
        private double currentVolume = 0.5;

        public TransportBar()
        {
            RelativeSizeAxes = Axes.X;
            Height = 66;
            Anchor = Anchor.BottomLeft;
            Origin = Anchor.BottomLeft;

            var dark = new Colour4(0x2a, 0x27, 0x33, 255);
            var darkHover = new Colour4(0x3a, 0x36, 0x47, 255);

            BarButton play, die, speedChip, mute;

            InternalChildren = new Drawable[]
            {
                new Box { RelativeSizeAxes = Axes.Both, Colour = new Colour4(0x17, 0x16, 0x1b, 245) },
                left = new Container
                {
                    Width = leftWidth,
                    RelativeSizeAxes = Axes.Y,
                    Anchor = Anchor.CentreLeft,
                    Origin = Anchor.CentreLeft,
                    X = edge,
                    Children = new Drawable[]
                    {
                        play = new BarButton(new Colour4(0x62, 0x45, 0xd9, 255), new Colour4(0x75, 0x5a, 0xe8, 255), new Vector2(buttonW, buttonH))
                        {
                            Anchor = Anchor.CentreLeft,
                            Origin = Anchor.CentreLeft,
                            TooltipText = "Play / pause",
                            Child = playIcon = new SpriteIcon { Icon = FontAwesome.Solid.Pause, Size = new Vector2(24), Anchor = Anchor.Centre, Origin = Anchor.Centre },
                        },
                        die = new BarButton(dark, darkHover, new Vector2(buttonW, buttonH))
                        {
                            Anchor = Anchor.CentreLeft,
                            Origin = Anchor.CentreLeft,
                            X = buttonW + gap,
                            TooltipText = "Play a different random map",
                            Child = new SpriteIcon { Icon = FontAwesome.Solid.Dice, Size = new Vector2(24), Anchor = Anchor.Centre, Origin = Anchor.Centre },
                        },
                        speedChip = new BarButton(dark, darkHover, new Vector2(speedW, buttonH))
                        {
                            Anchor = Anchor.CentreLeft,
                            Origin = Anchor.CentreLeft,
                            X = (buttonW + gap) * 2,
                            TooltipText = "Playback speed (click to change)",
                            Child = speedText = new OsuSpriteText { Font = OsuFont.GetFont(size: 28), Anchor = Anchor.Centre, Origin = Anchor.Centre, Text = "1x" },
                        },
                    },
                },
                right = new Container
                {
                    Width = rightWidth,
                    RelativeSizeAxes = Axes.Y,
                    Anchor = Anchor.CentreRight,
                    Origin = Anchor.CentreRight,
                    X = -edge,
                    Children = new Drawable[]
                    {
                        mute = new BarButton(dark, darkHover, new Vector2(buttonW, buttonH))
                        {
                            Anchor = Anchor.CentreLeft,
                            Origin = Anchor.CentreLeft,
                            TooltipText = "Mute",
                            Child = volumeIcon = new SpriteIcon { Icon = FontAwesome.Solid.VolumeUp, Size = new Vector2(24), Anchor = Anchor.Centre, Origin = Anchor.Centre },
                        },
                        volume = new BarSlider(1.5f)
                        {
                            Anchor = Anchor.CentreLeft,
                            Origin = Anchor.CentreLeft,
                            X = buttonW + gap,
                            Width = volumeW,
                            TooltipText = "Volume",
                        },
                        timeText = new OsuSpriteText { Font = OsuFont.GetFont(size: 28), Anchor = Anchor.CentreLeft, Origin = Anchor.CentreRight, X = rightWidth, Text = "0:00 / 0:00", Colour = new Colour4(0xcf, 0xcc, 0xdb, 255) },
                    },
                },
                seek = new BarSlider(1.5f) { Anchor = Anchor.CentreLeft, Origin = Anchor.CentreLeft, TooltipText = "Seek" },
            };

            play.Clicked = () => PlayPause?.Invoke();
            die.Clicked = () => NewMap?.Invoke();

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

                SetSpeed?.Invoke(next);
            };

            mute.Clicked = () =>
            {
                if (currentVolume > 0.001)
                {
                    lastVolume = currentVolume;
                    SetVolume?.Invoke(0);
                }
                else
                    SetVolume?.Invoke(lastVolume > 0.001 ? lastVolume : 0.1);
            };

            seek.Changed = f => Seek?.Invoke(f);
            volume.Changed = f => SetVolume?.Invoke(f);
        }

        // the bar takes the clicks that would pause the picture
        protected override bool OnClick(ClickEvent e) => true;
        protected override bool OnMouseDown(MouseDownEvent e) => true;
        protected override bool OnHover(HoverEvent e) => true;

        protected override void Update()
        {
            base.Update();
            seek.X = edge + leftWidth + groupGap;
            seek.Width = Math.Max(DrawWidth - (edge + leftWidth + groupGap) - (groupGap + rightWidth + edge), 20);
        }

        public void Refresh(bool playing, double fraction, string time, double volumeNow, double speedNow)
        {
            playIcon.Icon = playing ? FontAwesome.Solid.Pause : FontAwesome.Solid.Play;

            if (!seek.Dragging)
                seek.Value = fraction;

            timeText.Text = time;
            currentVolume = volumeNow;

            if (!volume.Dragging)
                volume.Value = volumeNow;

            volumeIcon.Icon = volumeNow <= 0.001 ? FontAwesome.Solid.VolumeMute : FontAwesome.Solid.VolumeUp;
            speed = speedNow;
            speedText.Text = (Math.Round(speedNow * 100) / 100).ToString("0.##") + "x";
        }
    }
}
