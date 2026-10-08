pub trait Scanner {
    fn id(&self) -> &'static str;
}

pub struct ScannerRegistry {
    scanners: Vec<Box<dyn Scanner + Send + Sync>>,
}

impl ScannerRegistry {
    pub fn new() -> Self {
        Self { scanners: Vec::new() }
    }

    pub fn register(&mut self, scanner: Box<dyn Scanner + Send + Sync>) {
        self.scanners.push(scanner);
    }

    pub fn len(&self) -> usize {
        self.scanners.len()
    }
}
