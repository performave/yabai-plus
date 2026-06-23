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
}
