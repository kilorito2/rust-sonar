//! Reverberación de sala pequeña (Schroeder: 4 filtros peine en paralelo + 2 pasa-todo en serie
//! por lado). Es lo que da "inmersión" al audio espacial: sin reflexiones, el sonido en unos
//! auriculares se percibe dentro de la cabeza.

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    feedback: f32,
    damp: f32,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len], pos: 0, feedback: 0.72, damp: 0.35, store: 0.0 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        // Paso bajo en la realimentación: las paredes absorben antes los agudos.
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        self.buf[self.pos] = x + self.store * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

struct AllPass {
    buf: Vec<f32>,
    pos: usize,
}

impl AllPass {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len], pos: 0 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.pos];
        let y = delayed - x;
        self.buf[self.pos] = x + delayed * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

struct Side {
    combs: Vec<Comb>,
    allpasses: Vec<AllPass>,
}

impl Side {
    fn new(offset: usize) -> Self {
        Self {
            combs: [1557, 1617, 1491, 1422].iter().map(|n| Comb::new(n + offset)).collect(),
            allpasses: [225, 556].iter().map(|n| AllPass::new(n + offset)).collect(),
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let mut y: f32 = self.combs.iter_mut().map(|c| c.process(x)).sum();
        for a in &mut self.allpasses {
            y = a.process(y);
        }
        y * 0.25
    }
}

pub struct Reverb {
    left: Side,
    right: Side,
}

impl Reverb {
    pub fn new() -> Self {
        // Longitudes distintas en cada lado para que la cola no suene mono.
        Self { left: Side::new(0), right: Side::new(23) }
    }

    /// Entrada estéreo, salida solo la señal reverberada (sin la directa).
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        (self.left.process(l), self.right.process(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_produces_a_decaying_tail() {
        let mut rv = Reverb::new();
        let mut energy = Vec::new();
        let mut acc = 0.0;
        for i in 0..48_000 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = rv.process(x, x);
            acc += l * l + r * r;
            if i % 12_000 == 11_999 {
                energy.push(acc);
                acc = 0.0;
            }
        }
        assert!(energy[0] > 0.0, "tiene que sonar algo");
        assert!(energy[3] < energy[0] * 0.05, "la cola debe apagarse: {energy:?}");
    }
}
