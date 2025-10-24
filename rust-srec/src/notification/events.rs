use crate::domain::job::Job;
use crate::domain::live_session::LiveSession;

#[derive(Clone, Debug)]
pub enum SystemEvent {
    DownloadCompleted(LiveSession, Job),
    FatalError(String),
}