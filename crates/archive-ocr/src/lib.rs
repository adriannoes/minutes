//! Reading text out of scanned pages, with Apple's Vision framework.
//!
//! Runs entirely on the machine. No network, no model download, no LLM. An LLM
//! is not a near miss here, it is the opposite of the thing: it would mean
//! sending a client's documents to a service, which is the single behaviour
//! this application exists to avoid.
//!
//! # What this produces is not evidence
//!
//! Everything else in this tool hands back characters the author wrote. This
//! hands back a machine's reading of an image, and on the material a long
//! practice actually accumulates -- faxes, stamped exhibits, photocopied
//! signature pages -- the reading is sometimes wrong. Output therefore travels
//! as `TranscribedCard`, which is not an `EvidenceCard`, cannot be converted
//! into one, and cannot answer a same-clause question. See
//! `minutes_archive_core::retrieval::TextProvenance`.
//!
//! # Containment
//!
//! Measured, not assumed: Vision cannot run under the read-scoping the
//! converter and semantic workers use. Bisecting the profile against a real
//! image, it still fails with `iokit-open`, with `/private/var/db`, and with
//! `/Library` and `/private/var/folders` added; it works only once filesystem
//! reads are allowed broadly. Scoped reads plus full `iokit*` still fail, so it
//! is the read scope and not IOKit.
//!
//! That is a real step down from the other two workers, and it is stated
//! plainly rather than buried in a profile diff. What still holds:
//!
//! - `(deny default)` and `(deny network*)`
//! - no write anywhere except `/dev/fd`
//! - the mach-lookup denylist that keeps pasteboard, distributed
//!   notifications, logd, diagnosticd, analyticsd, ReportCrash and
//!   launchservicesd unreachable
//! - `RLIMIT_AS` and `RLIMIT_CPU`, bound before the decoder sees a byte
//! - one process per document, which exits when the page is read
//!
//! The reason the read capability is containable rather than fatal: this
//! process has no way to send anything anywhere. It cannot open a socket and
//! cannot write a file. Its only channel out is stdout, which the parent reads
//! as length-prefixed JSON against a fixed schema. A compromised worker could
//! read files it has no business reading, but the only way to move what it read
//! is to smuggle it through recognised text, which then appears to the reader
//! as visible nonsense on a card labelled as a machine's reading.
//!
//! Image decoders are a classic attack surface and these bytes are attacker
//! controlled, so the hostile corpus is part of the crate rather than an
//! afterthought.

mod bounded;
pub use bounded::{BoundedTranscriber, WORKER_MARKER};

#[cfg(target_os = "macos")]
use std::ffi::{c_char, CStr};
#[cfg(target_os = "macos")]
use std::ptr;

use serde::{Deserialize, Serialize};

/// Identifies the recogniser on every card it produces, so a transcription can
/// be traced to what read it. The revision is Vision's own, pinned rather than
/// taken as "latest", for the same reason the semantic model is pinned.
pub const TRANSCRIBER: &str = "apple-vision-text-r3";

/// Refuse a page that would take longer than a reader would wait, and cap what
/// one image can turn into.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_LINES: usize = 20_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecognizedLine {
    pub text: String,
    /// Vision's own confidence for this line, 0.0..=1.0.
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecognizedPage {
    pub lines: Vec<RecognizedLine>,
}

impl RecognizedPage {
    /// The weakest line in the page.
    ///
    /// A page is only as trustworthy as its worst line, and a reader deciding
    /// whether to go and check the original is better served by the floor than
    /// by an average that a wall of clean body text would flatter.
    pub fn lowest_confidence(&self) -> f32 {
        self.lines
            .iter()
            .map(|line| line.confidence)
            .fold(f32::INFINITY, f32::min)
            // An empty page folds to infinity, and a recogniser is not
            // required to stay in range, so the result is clamped rather than
            // trusted.
            .clamp(0.0, 1.0)
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum OcrError {
    #[error("the image was empty or larger than this reader accepts")]
    ImageRefused,
    #[error("the image could not be decoded")]
    MalformedImage,
    #[error("the recognizer produced more text than one page should hold")]
    OutputBudgetExceeded,
    #[error("the text recognizer is unavailable on this Mac")]
    RecognizerUnavailable,
    #[error("the OCR worker security boundary could not be installed")]
    SecurityBoundaryUnavailable,
}

#[cfg(target_os = "macos")]
#[link(name = "System")]
unsafe extern "C" {
    fn sandbox_init(profile: *const c_char, flags: u64, errorbuf: *mut *mut c_char) -> i32;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// See the containment note at the top of the file for why the read scope is
/// wider here than in the sibling workers, and what still holds.
///
/// `(deny network*)` is redundant given `(deny default)` and is kept as a
/// statement of intent: removing it changes nothing, which the self-test
/// confirms by continuing to pass, so it is not load-bearing and this says so
/// rather than leaving a reader to assume a test covers it. The write denial
/// IS load-bearing -- allowing `/private/tmp` fails the self-test.
#[cfg(target_os = "macos")]
const PROFILE: &CStr = c"(version 1)
(deny default)
(deny network*)
(allow process-info-pidinfo (target self))
(allow sysctl-read)
(allow iokit-open)
(allow mach-lookup)
(deny mach-lookup (global-name \"com.apple.pasteboard.1\"))
(deny mach-lookup (global-name \"com.apple.distributed_notifications@1v3\"))
(deny mach-lookup (global-name \"com.apple.distributed_notifications@Uv3\"))
(deny mach-lookup (global-name \"com.apple.system.logger\"))
(deny mach-lookup (global-name \"com.apple.logd\"))
(deny mach-lookup (global-name \"com.apple.logd.events\"))
(deny mach-lookup (global-name \"com.apple.diagnosticd\"))
(deny mach-lookup (global-name \"com.apple.analyticsd\"))
(deny mach-lookup (global-name \"com.apple.ReportCrash\"))
(deny mach-lookup (global-name \"com.apple.coreservices.launchservicesd\"))
(deny mach-lookup (global-name \"com.apple.system.notification_center\"))
(allow file-read*)
(allow file-write-data (subpath \"/dev/fd\"))
";

#[cfg(target_os = "macos")]
pub fn install_worker_security_boundary() -> Result<(), OcrError> {
    let mut error_buffer = ptr::null_mut();
    let status = unsafe { sandbox_init(PROFILE.as_ptr(), 0, &mut error_buffer) };
    if status != 0 {
        if !error_buffer.is_null() {
            unsafe { sandbox_free_error(error_buffer) };
        }
        return Err(OcrError::SecurityBoundaryUnavailable);
    }
    bind_resource_limits()
}

#[cfg(not(target_os = "macos"))]
pub fn install_worker_security_boundary() -> Result<(), OcrError> {
    Err(OcrError::RecognizerUnavailable)
}

/// Bound before any image byte is decoded, so a crafted image cannot exhaust
/// the machine before the limit applies.
///
/// The address-space ceiling is relative to what this process already maps,
/// not an absolute figure. A macOS process starts with a very large virtual
/// size -- the dyld shared cache alone accounts for most of it -- so an
/// absolute limit that sounds generous is in fact below current usage, and
/// `setrlimit` fails or the process dies immediately. Getting that wrong is
/// what made the first version of this worker exit 70 before it read anything.
#[cfg(target_os = "macos")]
fn bind_resource_limits() -> Result<(), OcrError> {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::task::task_info;
    use mach2::task_info::{
        task_basic_info_64, task_info_t, TASK_BASIC_INFO_64, TASK_BASIC_INFO_64_COUNT,
    };
    use mach2::traps::mach_task_self;

    // Vision decodes a full-page image and runs a recognition model, so it is
    // allowed more headroom than a text parser, and a longer slice of CPU than
    // the converter because a dense page genuinely takes seconds.
    const MEMORY_GROWTH_BYTES: u64 = 3 * 1024 * 1024 * 1024;
    const CPU_SECONDS: libc::rlim_t = 120;

    let cpu = libc::rlimit {
        rlim_cur: CPU_SECONDS,
        rlim_max: CPU_SECONDS,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0 {
        return Err(OcrError::SecurityBoundaryUnavailable);
    }

    let mut info = task_basic_info_64::default();
    let mut count = TASK_BASIC_INFO_64_COUNT;
    let status = unsafe {
        task_info(
            mach_task_self(),
            TASK_BASIC_INFO_64,
            (&mut info as *mut task_basic_info_64).cast::<libc::c_int>() as task_info_t,
            &mut count,
        )
    };
    if status != KERN_SUCCESS || count != TASK_BASIC_INFO_64_COUNT {
        return Err(OcrError::SecurityBoundaryUnavailable);
    }
    let limit = info
        .virtual_size
        .checked_add(MEMORY_GROWTH_BYTES)
        .ok_or(OcrError::SecurityBoundaryUnavailable)?;
    let address_space = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &address_space) } != 0 {
        return Err(OcrError::SecurityBoundaryUnavailable);
    }
    Ok(())
}

/// Read the text in one page image.
///
/// Call only inside a worker that has already installed the boundary above.
#[cfg(target_os = "macos")]
pub fn recognize_page(image: &[u8]) -> Result<RecognizedPage, OcrError> {
    use objc2::rc::Retained;
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest,
    };

    if image.is_empty() || image.len() > MAX_IMAGE_BYTES {
        return Err(OcrError::ImageRefused);
    }

    // Vision is an Objective-C API reached through `objc2`; every call below is
    // a message send whose receiver was created immediately above it.
    let page = unsafe {
        let data = NSData::with_bytes(image);
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &NSDictionary::new(),
        );
        let request = VNRecognizeTextRequest::new();
        let requests: Retained<NSArray<VNRequest>> =
            NSArray::from_retained_slice(&[Retained::cast_unchecked(request.clone())]);
        handler
            .performRequests_error(&requests)
            .map_err(|_| OcrError::MalformedImage)?;
        let Some(results) = request.results() else {
            // A page with no text is a legitimate outcome, not a failure: a
            // photograph, a blank scan, a separator sheet.
            return Ok(RecognizedPage { lines: Vec::new() });
        };

        let mut lines = Vec::new();
        let mut text_bytes = 0usize;
        for index in 0..results.count() {
            if lines.len() >= MAX_LINES {
                return Err(OcrError::OutputBudgetExceeded);
            }
            let observation: Retained<VNRecognizedTextObservation> =
                Retained::cast_unchecked(results.objectAtIndex(index));
            let Some(candidate) = observation.topCandidates(1).firstObject() else {
                continue;
            };
            let text = candidate.string().to_string();
            if text.trim().is_empty() {
                continue;
            }
            text_bytes = text_bytes
                .checked_add(text.len())
                .ok_or(OcrError::OutputBudgetExceeded)?;
            if text_bytes > MAX_TEXT_BYTES {
                return Err(OcrError::OutputBudgetExceeded);
            }
            lines.push(RecognizedLine {
                text,
                confidence: candidate.confidence(),
            });
        }
        RecognizedPage { lines }
    };
    Ok(page)
}

#[cfg(not(target_os = "macos"))]
pub fn recognize_page(_image: &[u8]) -> Result<RecognizedPage, OcrError> {
    Err(OcrError::RecognizerUnavailable)
}

/// Refuse to run at all if the boundary is not what it claims to be.
///
/// Probes named and unnamed resources, the way the sibling workers' tests do.
/// The read allowance is deliberately wide here, so the things this asserts are
/// the ones that still hold: no network, and no write anywhere the parent could
/// later read.
#[cfg(target_os = "macos")]
pub fn sandbox_self_test() -> i32 {
    let network_denied = std::net::TcpListener::bind("127.0.0.1:0").is_err()
        && std::net::TcpStream::connect("127.0.0.1:1").is_err();
    let write_denied = [
        "/private/tmp",
        "/private/var/tmp",
        "/Users/Shared",
        "/Library/Caches",
    ]
    .iter()
    .all(|directory| {
        let probe = std::path::Path::new(directory).join("minutes-archive-ocr-probe");
        let denied = std::fs::write(&probe, b"probe").is_err();
        if !denied {
            let _ = std::fs::remove_file(&probe);
        }
        denied
    });
    if network_denied && write_denied {
        0
    } else {
        71
    }
}

#[cfg(not(target_os = "macos"))]
pub fn sandbox_self_test() -> i32 {
    71
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_only_as_good_as_its_worst_line() {
        let page = RecognizedPage {
            lines: vec![
                RecognizedLine {
                    text: "CONFIDENTIALITY".into(),
                    confidence: 1.0,
                },
                RecognizedLine {
                    text: "sha11 pr0tect".into(),
                    confidence: 0.31,
                },
            ],
        };
        // Not an average: a wall of clean body text would otherwise hide the
        // one line the reader most needs to go and check.
        assert!((page.lowest_confidence() - 0.31).abs() < f32::EPSILON);
    }

    #[test]
    fn an_empty_page_reports_no_confidence_above_the_range() {
        let page = RecognizedPage { lines: Vec::new() };
        let lowest = page.lowest_confidence();
        assert!((0.0..=1.0).contains(&lowest), "got {lowest}");
    }

    #[test]
    fn an_empty_or_oversized_image_is_refused_before_decoding() {
        assert_eq!(recognize_page(b""), Err(OcrError::ImageRefused));
        let oversized = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert_eq!(recognize_page(&oversized), Err(OcrError::ImageRefused));
    }
}
