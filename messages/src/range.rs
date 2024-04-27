use std::cmp::{max, Ordering};

pub use crate::{Range, RangeSet};

impl Range {
    pub fn new(begin: u32, end: u32) -> Self {
        assert!(begin <= end);
        Range { begin, end }
    }
}

impl RangeSet {
    pub fn empty() -> Self {
        Default::default()
    }

    pub fn has(&self, point: u32) -> bool {
        self.containing_range(point).is_some()
    }

    pub fn find_containing_range(&self, point: u32) -> Option<Range> {
        self.containing_range(point).map(|i| self.ranges[i])
    }

    fn containing_range(&self, point: u32) -> Option<usize> {
        self.ranges
            .binary_search_by(|i| {
                if point < i.begin {
                    Ordering::Greater
                } else if i.end < point {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .ok()
    }

    pub fn includes(&self, range: Range) -> bool {
        if let Some(c) = self.find_containing_range(range.begin) {
            c.end >= range.end
        } else {
            false
        }
    }
}

impl<T: IntoIterator<Item = Range>> From<T> for RangeSet {
    fn from(iter: T) -> Self {
        let mut range_set = RangeSet::empty();
        range_set.extend(iter);
        range_set
    }
}

impl Extend<Range> for RangeSet {
    fn extend<T: IntoIterator<Item = Range>>(&mut self, iter: T) {
        self.ranges.extend(iter);
        self.ranges.sort();
        let mut pi = 0;
        for i in 1..self.ranges.len() {
            let c = self.ranges[i];
            let p = self.ranges[pi];
            if c.begin > p.end + 1 {
                pi += 1;
                self.ranges[pi] = c;
            } else {
                self.ranges[pi] = Range::new(p.begin, max(c.end, p.end));
            }
        }
        self.ranges.truncate(pi + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::{Range, RangeSet};

    #[test]
    fn range_set_always_sorted_and_separated() {
        let rs = RangeSet::from(vec![
            Range::new(10, 20),
            Range::new(21, 30),
            Range::new(35, 40),
            Range::new(5, 10),
        ]);
        assert_eq!(rs.ranges, vec![Range::new(5, 30), Range::new(35, 40)]);
    }

    #[test]
    fn range_set_has() {
        let rs = RangeSet::from(vec![Range::new(0, 10), Range::new(20, 30), Range::new(40, 50)]);

        assert!(rs.has(0));
        assert!(rs.has(5));
        assert!(rs.has(10));
        assert!(rs.has(21));
        assert!(rs.has(40));
        assert!(rs.has(41));
        assert!(rs.has(50));
        assert!(!rs.has(11));
        assert!(!rs.has(19));
        assert!(!rs.has(35));
    }

    #[test]
    fn range_set_find_containing_range() {
        let rs = RangeSet::from(vec![Range::new(0, 10), Range::new(20, 30), Range::new(40, 50)]);

        assert_eq!(rs.find_containing_range(0), Some(Range::new(0, 10)));
        assert_eq!(rs.find_containing_range(5), Some(Range::new(0, 10)));
        assert_eq!(rs.find_containing_range(30), Some(Range::new(20, 30)));
        assert_eq!(rs.find_containing_range(41), Some(Range::new(40, 50)));
        assert_eq!(rs.find_containing_range(15), None);
    }
}
