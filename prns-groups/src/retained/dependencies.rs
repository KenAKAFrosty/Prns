use super::*;
impl Store<'_> {
    pub(crate) fn dependencies(&self, report: &mut crate::Dependencies) -> Result<(), Error> {
        let header = self.snapshot()?.header;
        crate::dependencies::add(
            &mut report.terminal_operations,
            header.count - header.active,
        )?;
        crate::dependencies::add(&mut report.unresolved_operations, header.active)?;
        crate::dependencies::add(
            &mut report.passive_fences,
            u64::from(header.retired_before != 0),
        )
    }
}
