//! A sheet instance: one printed side of one booklet (template page + form + serial, §6.7).

use crate::geometry::{ECHO_FONT, ECHO_W, TAG_SET_ANSWER, TAG_SET_PROOF};
use crate::layout::{PageGeometry, Template, approx_text_width};
use crate::pagecode::{ExamKey, PageCode, PageCodeError};
use omr_model::{BookletSerial, FormCode, PageNo};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Per-booklet personalisation printed in the header (§6.7): the student's name and
/// institutional student ID. **Never** a national ID or Iqama number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Personalisation {
    name: String,
    student_id: String,
}

impl Personalisation {
    /// Builds a personalisation, refusing anything that looks like a Saudi national ID or
    /// Iqama number (§6.7, §9.2): 10 digits starting with 1 or 2 with a valid Luhn check.
    pub fn new(
        name: impl Into<String>,
        student_id: impl Into<String>,
    ) -> Result<Self, PersonalisationError> {
        let student_id = student_id.into();
        if looks_like_national_id(&student_id) {
            return Err(PersonalisationError::NationalIdRefused);
        }
        if student_id.chars().count() > 20 {
            return Err(PersonalisationError::TooLong);
        }
        let name = name.into();
        if name.chars().count() > 80 {
            return Err(PersonalisationError::TooLong);
        }
        Ok(Self { name, student_id })
    }

    /// The printed name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The printed institutional student ID.
    pub fn student_id(&self) -> &str {
        &self.student_id
    }
}

/// Why a personalisation was refused.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PersonalisationError {
    /// The "student ID" has the shape of a national ID or Iqama number.
    #[error("refusing to print a national ID or Iqama number on a sheet (§6.7)")]
    NationalIdRefused,
    /// Name or ID too long for the header slot.
    #[error("name or student ID too long for the header")]
    TooLong,
}

/// Value of an ASCII, Arabic-Indic (٠–٩) or Extended Arabic-Indic (۰–۹) digit.
fn digit_value(c: char) -> Option<u32> {
    match c {
        '0'..='9' => c.to_digit(10),
        '\u{0660}'..='\u{0669}' => Some(u32::from(c) - 0x0660),
        '\u{06F0}'..='\u{06F9}' => Some(u32::from(c) - 0x06F0),
        _ => None,
    }
}

/// True for a Saudi national ID (starts with 1) or Iqama (starts with 2): 10 digits, Luhn-valid.
///
/// Digits are read in any of the three digit scripts and everything else (spaces, hyphens,
/// prefixes) is ignored, so `1000-000-008` or `١٠٠٠٠٠٠٠٠٨` are caught too.
pub fn looks_like_national_id(s: &str) -> bool {
    let d: Vec<u32> = s.chars().filter_map(digit_value).collect();
    if d.len() != 10 || !matches!(d[0], 1 | 2) {
        return false;
    }
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            if i % 2 == 1 {
                let y = x * 2;
                if y > 9 { y - 9 } else { y }
            } else {
                x
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// One printed side of one booklet.
#[derive(Clone, Debug)]
pub struct SheetInstance<'a> {
    /// The template revision.
    pub template: &'a Template,
    /// Form (variant), 1 = A.
    pub form: FormCode,
    /// Page side, 1-based.
    pub page: PageNo,
    /// Booklet serial (0 = generic copy).
    pub serial: BookletSerial,
    /// The exam's page-code key.
    pub key: &'a ExamKey,
    /// Optional personalisation.
    pub personal: Option<Personalisation>,
    /// Draft proof: PROOF watermark and the proof tag set, so it can never be graded (§6.1).
    pub proof: bool,
}

/// Why an instance is invalid.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InstanceError {
    /// The page does not exist in the template.
    #[error("page {0} does not exist")]
    NoSuchPage(u16),
    /// The form does not exist in the template.
    #[error("form {0} does not exist")]
    NoSuchForm(u16),
    /// The page code could not be encoded.
    #[error(transparent)]
    PageCode(#[from] PageCodeError),
}

impl<'a> SheetInstance<'a> {
    /// A generic (serial 0), unpersonalised, final sheet.
    pub fn new(template: &'a Template, key: &'a ExamKey, form: FormCode, page: PageNo) -> Self {
        Self {
            template,
            form,
            page,
            serial: BookletSerial::GENERIC,
            key,
            personal: None,
            proof: false,
        }
    }

    /// Sets the booklet serial.
    pub fn with_serial(mut self, serial: BookletSerial) -> Self {
        self.serial = serial;
        self
    }

    /// Sets the personalisation.
    pub fn with_personal(mut self, p: Personalisation) -> Self {
        self.personal = Some(p);
        self
    }

    /// Checks page and form exist.
    pub fn check(&self) -> Result<(), InstanceError> {
        if self.template.page(self.page).is_none() {
            return Err(InstanceError::NoSuchPage(self.page.0));
        }
        if self.form.0 == 0 || self.form.0 > self.template.spec().forms {
            return Err(InstanceError::NoSuchForm(self.form.0));
        }
        Ok(())
    }

    /// The page geometry.
    pub fn geometry(&self) -> Result<&'a PageGeometry, InstanceError> {
        self.template
            .page(self.page)
            .ok_or(InstanceError::NoSuchPage(self.page.0))
    }

    /// The page-code fields.
    pub fn page_code(&self) -> PageCode {
        self.template.page_code(self.form, self.page, self.serial)
    }

    /// The 41-character page-code payload.
    pub fn payload(&self) -> Result<String, InstanceError> {
        self.check()?;
        Ok(self.page_code().encode(self.key)?)
    }

    /// "Form B · Page 1/2".
    pub fn form_page_text(&self) -> String {
        format!(
            "Form {} · Page {}/{}",
            self.form.letter(),
            self.page.0,
            self.template.page_count()
        )
    }

    /// The echo line's friendly prefix: course, form and page. The course is cut (at most 18
    /// characters) so the line fits the echo box with a 1 mm margin for metric error.
    pub fn echo_prefix(&self) -> String {
        let fp = self.form_page_text();
        let course = self.template.spec().course.trim();
        if course.is_empty() {
            return fp;
        }
        let avail =
            ECHO_W - 1_000 - approx_text_width(&format!(" · {fp}"), ECHO_FONT, false, false);
        let mut c = String::new();
        for ch in course.chars().take(18) {
            c.push(ch);
            if approx_text_width(&c, ECHO_FONT, false, false) > avail {
                c.pop();
                break;
            }
        }
        let c = c.trim();
        if c.is_empty() {
            fp
        } else {
            format!("{c} · {fp}")
        }
    }

    /// AprilTag set printed on this side (0 = answer page, 3 = proof).
    pub fn tag_set(&self) -> u16 {
        if self.proof {
            TAG_SET_PROOF
        } else {
            TAG_SET_ANSWER
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::TemplateSpec;
    use omr_model::{CancelMode, ExamId};

    #[test]
    fn national_ids_are_refused() {
        // 1000000008 is Luhn-valid and starts with 1.
        assert!(looks_like_national_id("1000000008"));
        assert!(looks_like_national_id("2000000006"));
        assert!(!looks_like_national_id("1000000009"), "bad check digit");
        assert!(!looks_like_national_id("441234567"), "9-digit student ID");
        assert!(!looks_like_national_id("4412345678"), "starts with 4");
        // Separators, prefixes and Arabic-Indic digits do not hide one.
        assert!(looks_like_national_id("1000-000-008"));
        assert!(looks_like_national_id(" 1000 000 008 "));
        assert!(looks_like_national_id("ID 1000000008"));
        assert!(looks_like_national_id("١٠٠٠٠٠٠٠٠٨"));
        assert!(looks_like_national_id("۲۰۰۰۰۰۰۰۰۶"));
        assert_eq!(
            Personalisation::new("Sara", "١٠٠٠-٠٠٠-٠٠٨"),
            Err(PersonalisationError::NationalIdRefused)
        );
        assert_eq!(
            Personalisation::new("Sara", "1000000008"),
            Err(PersonalisationError::NationalIdRefused)
        );
        assert!(Personalisation::new("سارة", "441234567").is_ok());
    }

    #[test]
    fn instance_texts_and_payload() {
        let mut s = TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(5)),
            20,
            4,
            CancelMode::EraseOnly,
        );
        s.forms = 2;
        let t = Template::build(s).unwrap();
        let k = ExamKey::from_bytes([3; 32]);
        let i = SheetInstance::new(&t, &k, FormCode(2), PageNo(1)).with_serial(BookletSerial(77));
        assert_eq!(i.form_page_text(), "Form B · Page 1/1");
        let p = i.payload().unwrap();
        assert_eq!(
            crate::pagecode::decode_verified(&p, &k).unwrap().serial,
            BookletSerial(77)
        );
        let bad = SheetInstance::new(&t, &k, FormCode(3), PageNo(1));
        assert_eq!(bad.payload(), Err(InstanceError::NoSuchForm(3)));
    }

    #[test]
    fn echo_prefix_fits_the_echo_box() {
        let mut s = TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(5)),
            20,
            4,
            CancelMode::EraseOnly,
        );
        s.course = "CHEMISTRY 101 SECTION 7 GENERAL".into();
        s.forms = 26;
        let t = Template::build(s).unwrap();
        let k = ExamKey::from_bytes([3; 32]);
        let i = SheetInstance::new(&t, &k, FormCode(23), PageNo(1));
        let p = i.echo_prefix();
        assert!(p.starts_with("CHEMISTRY"), "{p}");
        assert!(p.ends_with("Form W · Page 1/1"), "{p}");
        assert!(approx_text_width(&p, ECHO_FONT, false, false) <= ECHO_W - 1_000);
        // A short course is kept whole.
        let t2 = Template::build(TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(5)),
            20,
            4,
            CancelMode::EraseOnly,
        ))
        .unwrap();
        let i2 = SheetInstance::new(&t2, &k, FormCode(1), PageNo(1));
        assert_eq!(i2.echo_prefix(), "COURSE 101 · Form A · Page 1/1");
    }
}
