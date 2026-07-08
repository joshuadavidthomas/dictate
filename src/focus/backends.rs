mod niri;

use super::FocusObservation;
use super::SessionEnvironment;

pub(super) fn observe(environment: &SessionEnvironment) -> FocusObservation {
    if environment.is_niri() {
        niri::observe(environment)
    } else {
        FocusObservation::UnsupportedSession
    }
}
