#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    North,
    East,
    South,
    West,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Vertical,
    Horizontal,
}

impl Area {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn max_point(self) -> Point {
        Point {
            x: self.x + self.w - 1.0,
            y: self.y + self.h - 1.0,
        }
    }

    /// Whether `point` lies within this area (left/top inclusive, right/bottom
    /// exclusive), used to resolve the `mouse` selector against window/display
    /// frames.
    pub fn contains_point(self, point: Point) -> bool {
        point.x >= self.x
            && point.x < self.x + self.w
            && point.y >= self.y
            && point.y < self.y + self.h
    }

    pub fn split(self, split: Split, gap: i32, ratio: f32) -> (Self, Self) {
        let mut left = self;
        let mut right = self;
        let gap = gap as f32;

        match split {
            Split::Vertical => {
                let left_width = (self.w - gap) * ratio;
                let right_width = (self.w - gap) * (1.0 - ratio);

                left.w = truncate_like_c(left_width);
                right.w = truncate_like_c(right_width);
                right.x += truncate_like_c(left_width + 0.5) + gap;
            }
            Split::Horizontal => {
                let left_height = (self.h - gap) * ratio;
                let right_height = (self.h - gap) * (1.0 - ratio);

                left.h = truncate_like_c(left_height);
                right.h = truncate_like_c(right_height);
                right.y += truncate_like_c(left_height + 0.5) + gap;
            }
        }

        (left, right)
    }

    pub fn is_in_direction(self, target: Self, direction: Direction) -> bool {
        let source_max = self.max_point();
        let target_max = target.max_point();

        match direction {
            Direction::North if source_max.y <= target.y => return false,
            Direction::East if target_max.x <= self.x => return false,
            Direction::South if target_max.y <= self.y => return false,
            Direction::West if source_max.x <= target.x => return false,
            _ => {}
        }

        match direction {
            Direction::North | Direction::South => {
                (target_max.x > self.x && target_max.x <= source_max.x)
                    || (target.x < self.x && target_max.x > source_max.x)
                    || (target.x >= self.x && target.x < source_max.x)
            }
            Direction::East | Direction::West => {
                (target_max.y > self.y && target_max.y <= source_max.y)
                    || (target.y < self.y && target_max.y > source_max.y)
                    || (target.y >= self.y && target.y < source_max.y)
            }
        }
    }

    pub fn distance_in_direction(self, target: Self, direction: Direction) -> i32 {
        let source_max = self.max_point();
        let target_max = target.max_point();

        let distance = match direction {
            Direction::North => (target_max.y - self.y).abs(),
            Direction::East => (target.x - source_max.x).abs(),
            Direction::South => (target.y - source_max.y).abs(),
            Direction::West => (target_max.x - self.x).abs(),
        };

        distance as i32
    }
}

fn truncate_like_c(value: f32) -> f32 {
    (value as i32) as f32
}

/// The frame for `window --grid r:c:x:y:w:h` within `bounds` (a display's usable
/// area, C `display_bounds_constrained`), inset by `padding` (`[top, bottom, left,
/// right]`) and `gap`. Mirrors the cell math in the C
/// `window_manager_apply_grid`: the spec is clamped into range, the bounds are
/// inset by the space's padding and (per-edge) window gap, then the requested
/// `w`x`h` block of a `c`x`r` cell grid is measured back from the far edge so
/// rounding accumulates away from the origin exactly as the C does.
///
/// `spec` is `[r, c, x, y, w, h]` (rows, cols, cell col, cell row, col span, row
/// span), the order the `--grid` parser produces.
///
/// Divergence from C: `r`/`c` are clamped to at least 1 (C uses `unsigned`, so a
/// `0` there underflows); every other clamp matches.
pub fn grid_frame(bounds: Area, padding: [i32; 4], gap: i32, spec: [i32; 6]) -> Area {
    let [r, c, x, y, w, h] = spec;
    let r = r.max(1);
    let c = c.max(1);
    let x = x.clamp(0, c - 1);
    let y = y.clamp(0, r - 1);
    let w = w.max(1).min(c - x);
    let h = h.max(1).min(r - y);

    let [top, bottom, left, right] = padding;
    let mut bx = bounds.x + left as f32;
    let mut by = bounds.y + top as f32;
    let mut bw = bounds.w - (left + right) as f32;
    let mut bh = bounds.h - (top + bottom) as f32;

    let gap = gap as f32;
    if x > 0 {
        bx += gap;
        bw -= gap;
    }
    if y > 0 {
        by += gap;
        bh -= gap;
    }
    if c > x + w {
        bw -= gap;
    }
    if r > y + h {
        bh -= gap;
    }

    let cw = bw / c as f32;
    let ch = bh / r as f32;
    Area::new(
        bx + bw - cw * (c - x) as f32,
        by + bh - ch * (r - y) as f32,
        cw * w as f32,
        ch * h as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct TestArea {
        area: Area,
    }

    impl TestArea {
        fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
            Self {
                area: Area::new(x, y, w, h),
            }
        }
    }

    fn init_test_display_list() -> [TestArea; 3] {
        [
            TestArea::new(0.0, 0.0, 2560.0, 1440.0),
            TestArea::new(-1728.0, 0.0, 1728.0, 1117.0),
            TestArea::new(2560.0, 0.0, 1920.0, 1080.0),
        ]
    }

    fn closest_display_in_direction(
        display_list: &[TestArea],
        source: usize,
        direction: Direction,
    ) -> Option<usize> {
        let mut best_index = None;
        let mut best_distance = i32::MAX;

        for (index, display) in display_list.iter().enumerate() {
            if index == source {
                continue;
            }

            if display_list[source]
                .area
                .is_in_direction(display.area, direction)
            {
                let distance = display_list[source]
                    .area
                    .distance_in_direction(display.area, direction);
                if distance < best_distance {
                    best_index = Some(index);
                    best_distance = distance;
                }
            }
        }

        best_index
    }

    #[test]
    fn display_area_is_in_direction() {
        let display_list = init_test_display_list();

        assert!(
            display_list[0]
                .area
                .is_in_direction(display_list[1].area, Direction::West)
        );
        assert!(
            !display_list[0]
                .area
                .is_in_direction(display_list[1].area, Direction::East)
        );
        assert!(
            !display_list[0]
                .area
                .is_in_direction(display_list[2].area, Direction::West)
        );
        assert!(
            display_list[0]
                .area
                .is_in_direction(display_list[2].area, Direction::East)
        );
    }

    #[test]
    fn area_max_point_uses_inclusive_bounds() {
        let max = Area::new(10.0, 20.0, 50.0, 30.0).max_point();

        assert_eq!(max.x as i32, 59);
        assert_eq!(max.y as i32, 49);
    }

    #[test]
    fn area_is_in_vertical_direction() {
        let source = TestArea::new(0.0, 0.0, 100.0, 100.0);
        let north = TestArea::new(10.0, -80.0, 50.0, 50.0);
        let south = TestArea::new(10.0, 120.0, 50.0, 50.0);
        let north_east = TestArea::new(120.0, -80.0, 50.0, 50.0);

        assert!(source.area.is_in_direction(north.area, Direction::North));
        assert!(!source.area.is_in_direction(north.area, Direction::South));
        assert!(source.area.is_in_direction(south.area, Direction::South));
        assert!(
            !source
                .area
                .is_in_direction(north_east.area, Direction::North)
        );
    }

    #[test]
    fn area_make_pair_splits_with_gap() {
        let parent_y = Area::new(0.0, 0.0, 101.0, 50.0);
        let (left_y, right_y) = parent_y.split(Split::Vertical, 1, 0.5);

        assert_eq!(left_y.x as i32, 0);
        assert_eq!(left_y.w as i32, 50);
        assert_eq!(right_y.x as i32, 51);
        assert_eq!(right_y.w as i32, 50);

        let parent_x = Area::new(0.0, 0.0, 50.0, 101.0);
        let (left_x, right_x) = parent_x.split(Split::Horizontal, 1, 0.5);

        assert_eq!(left_x.y as i32, 0);
        assert_eq!(left_x.h as i32, 50);
        assert_eq!(right_x.y as i32, 51);
        assert_eq!(right_x.h as i32, 50);
    }

    #[test]
    fn closest_display_in_direction_matches_c_tests() {
        let display_list = init_test_display_list();

        assert_eq!(
            closest_display_in_direction(&display_list, 0, Direction::West),
            Some(1)
        );
        assert_eq!(
            closest_display_in_direction(&display_list, 1, Direction::West),
            None
        );
        assert_eq!(
            closest_display_in_direction(&display_list, 2, Direction::West),
            Some(0)
        );
        assert_eq!(
            closest_display_in_direction(&display_list, 0, Direction::East),
            Some(2)
        );
        assert_eq!(
            closest_display_in_direction(&display_list, 1, Direction::East),
            Some(0)
        );
        assert_eq!(
            closest_display_in_direction(&display_list, 2, Direction::East),
            None
        );
    }

    #[test]
    fn grid_frame_no_padding_no_gap() {
        let bounds = Area::new(0.0, 0.0, 1000.0, 800.0);
        // Full display: 1x1 grid, cell (0,0) 1x1 -> the whole bounds.
        assert_eq!(
            grid_frame(bounds, [0; 4], 0, [1, 1, 0, 0, 1, 1]),
            Area::new(0.0, 0.0, 1000.0, 800.0)
        );
        // 2x2 grid, top-left cell (col 0, row 0) -> left half, top half.
        assert_eq!(
            grid_frame(bounds, [0; 4], 0, [2, 2, 0, 0, 1, 1]),
            Area::new(0.0, 0.0, 500.0, 400.0)
        );
        // 2x2 grid, bottom-right cell (col 1, row 1) -> right half, bottom half.
        assert_eq!(
            grid_frame(bounds, [0; 4], 0, [2, 2, 1, 1, 1, 1]),
            Area::new(500.0, 400.0, 500.0, 400.0)
        );
        // A 2x1 block spanning both columns of the top row -> full width, top half.
        assert_eq!(
            grid_frame(bounds, [0; 4], 0, [2, 2, 0, 0, 2, 1]),
            Area::new(0.0, 0.0, 1000.0, 400.0)
        );
    }

    #[test]
    fn grid_frame_applies_padding_and_gap() {
        // 20px on every edge of padding, 10px gap; 2x2 grid, top-left cell.
        // Width after padding: 1000-40 = 960; the interior edge loses the 10px gap
        // -> 950 across two columns = 475 each. Height: 800-40 = 760, -10 = 750,
        // /2 = 375 each.
        let bounds = Area::new(0.0, 0.0, 1000.0, 800.0);
        assert_eq!(
            grid_frame(bounds, [20, 20, 20, 20], 10, [2, 2, 0, 0, 1, 1]),
            Area::new(20.0, 20.0, 475.0, 375.0)
        );
        // The bottom-right cell starts a gap past the midpoint (origin += gap).
        assert_eq!(
            grid_frame(bounds, [20, 20, 20, 20], 10, [2, 2, 1, 1, 1, 1]),
            Area::new(505.0, 405.0, 475.0, 375.0)
        );
    }

    #[test]
    fn grid_frame_clamps_out_of_range_spec() {
        let bounds = Area::new(0.0, 0.0, 1000.0, 800.0);
        // x/y past the grid clamp to the last cell; w/h past the edge clamp to fit.
        assert_eq!(
            grid_frame(bounds, [0; 4], 0, [2, 2, 5, 5, 9, 9]),
            grid_frame(bounds, [0; 4], 0, [2, 2, 1, 1, 1, 1])
        );
        // A degenerate 0x0 grid is treated as 1x1 (avoids divide-by-zero).
        assert_eq!(
            grid_frame(bounds, [0; 4], 0, [0, 0, 0, 0, 1, 1]),
            Area::new(0.0, 0.0, 1000.0, 800.0)
        );
    }
}
