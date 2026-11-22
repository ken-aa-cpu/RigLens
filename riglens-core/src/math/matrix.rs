use std::ops::Index;

#[derive(thiserror::Error, Clone, Debug)]
#[error("Couldn't construct matrix: not all lines have the same length ({0:?})")]
pub struct Matrix2dFromSliceVecsError(Vec<usize>);

#[derive(Clone, Debug, Default)]
pub struct Matrix2d<T> {
	width: usize,
	height: usize,
	transposed: bool,
	data: Vec<T>,
}

impl<T> Matrix2d<T> {
	pub fn width(&self) -> usize {
		self.width
	}

	pub fn height(&self) -> usize {
		self.height
	}

	pub fn is_transposed(&self) -> bool {
		self.transposed
	}

	pub fn get(&self, ix: usize, iy: usize) -> Option<&T> {
		let (ix, iy) = if self.transposed { (iy, ix) } else { (ix, iy) };
		if ix >= self.width || iy >= self.height {
			return None;
		}
		self.data.get(iy * self.width + ix)
	}

	pub fn get_mut(&mut self, ix: usize, iy: usize) -> Option<&mut T> {
		let (ix, iy) = if self.transposed { (iy, ix) } else { (ix, iy) };
		if ix >= self.width || iy >= self.height {
			return None;
		}
		self.data.get_mut(iy * self.width + ix)
	}

	pub fn any(&self, f: impl FnMut(&T) -> bool) -> bool {
		self.data.iter().any(f)
	}

	pub fn to_vec_vecs(&self) -> Vec<Vec<T>>
	where
		T: Clone,
	{
		(0..self.height)
			.map(|row| self.data[row * self.width..(row + 1) * self.width].to_vec())
			.collect()
	}

	pub fn from_grid_transposed(x_len: usize, y_len: usize, mut f: impl FnMut(usize, usize) -> T) -> Self {
		let mut data = Vec::with_capacity(x_len * y_len);
		for x in 0..x_len {
			for y in 0..y_len {
				data.push(f(x, y));
			}
		}
		Self {
			width: y_len,
			height: x_len,
			transposed: true,
			data,
		}
	}
}

impl<T> Index<(usize, usize)> for Matrix2d<T> {
	type Output = T;

	fn index(&self, (ix, iy): (usize, usize)) -> &Self::Output {
		let (ix, iy) = if self.transposed { (iy, ix) } else { (ix, iy) };

		if ix >= self.width || iy >= self.height {
			panic!(
				"Point index out of bounds: ({}, {}) > ({}, {})",
				ix, iy, self.width, self.height
			);
		}

		&self.data[iy * self.width + ix]
	}
}

impl<T: Default + Clone> Matrix2d<T> {
	pub fn default_filled(width: usize, height: usize, transposed: bool) -> Self {
		Self {
			width,
			height,
			transposed,
			data: vec![T::default(); width * height],
		}
	}

	pub fn from_slice_vecs(ss: &[Vec<T>], transposed: bool) -> Result<Self, Matrix2dFromSliceVecsError> {
		let height = ss.len();

		if height == 0 {
			Ok(Self {
				width: 0,
				height: 0,
				transposed,
				data: Vec::new(),
			})
		} else {
			let width = ss[0].len();

			if !ss.iter().all(|line| line.len() == width) {
				return Err(Matrix2dFromSliceVecsError(ss.iter().map(|line| line.len()).collect()));
			}

			let mut data = Vec::with_capacity(width * height);
			for line in ss {
				data.extend_from_slice(line);
			}

			Ok(Self {
				width,
				height,
				transposed,
				data,
			})
		}
	}
}
