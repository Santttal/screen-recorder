use std::os::fd::RawFd;

use ashpd::desktop::screencast::{
    CursorMode, PersistMode, Screencast, SourceType, Stream, Streams,
};
use ashpd::desktop::{ResponseError, Session};
use ashpd::{Error as AshpdError, WindowIdentifier};
use enumflags2::BitFlags;

use crate::config::{CaptureSource, CursorMode as SettingsCursorMode};

fn map_cursor_mode(m: SettingsCursorMode) -> CursorMode {
    match m {
        SettingsCursorMode::Hidden => CursorMode::Hidden,
        SettingsCursorMode::Embedded => CursorMode::Embedded,
        SettingsCursorMode::Metadata => CursorMode::Metadata,
    }
}

fn source_flags(cs: CaptureSource) -> BitFlags<SourceType> {
    match cs {
        CaptureSource::Screen => SourceType::Monitor.into(),
        CaptureSource::Window => SourceType::Window.into(),
    }
}

pub struct ScreenCastSession {
    _proxy: Screencast<'static>,
    session: Session<'static>,
    pub streams: Vec<Stream>,
    pub pipewire_fd: RawFd,
    pub restore_token: Option<String>,
}

#[allow(dead_code)]
pub enum OpenOutcome {
    Opened(ScreenCastSession),
    Cancelled,
}

impl ScreenCastSession {
    pub async fn open(
        parent: WindowIdentifier,
        restore_token: Option<String>,
        cursor_mode: SettingsCursorMode,
        capture_source: CaptureSource,
    ) -> ashpd::Result<Self> {
        let proxy = Screencast::new().await?;
        let session = proxy.create_session().await?;

        let types: BitFlags<SourceType> = source_flags(capture_source);
        let select_req = proxy
            .select_sources(
                &session,
                map_cursor_mode(cursor_mode),
                types,
                false,
                restore_token.as_deref(),
                PersistMode::ExplicitlyRevoked,
            )
            .await?;
        select_req.response()?;

        let start_req = proxy.start(&session, &parent).await?;
        let streams_response: Streams = start_req.response()?;
        let streams: Vec<Stream> = streams_response.streams().to_vec();
        let restore_token = streams_response.restore_token().map(|s| s.to_owned());

        let pipewire_fd = proxy.open_pipe_wire_remote(&session).await?;

        Ok(Self {
            _proxy: proxy,
            session,
            streams,
            pipewire_fd,
            restore_token,
        })
    }

    pub fn primary_node_id(&self) -> Option<u32> {
        self.streams.first().map(|s| s.pipe_wire_node_id())
    }

    pub fn primary_size(&self) -> Option<(i32, i32)> {
        self.streams.first().and_then(|s| s.size())
    }

    pub async fn close(self) -> ashpd::Result<()> {
        self.session.close().await
    }
}

pub async fn open_or_cancel(
    parent: WindowIdentifier,
    restore_token: Option<String>,
    cursor_mode: SettingsCursorMode,
    capture_source: CaptureSource,
) -> ashpd::Result<OpenOutcome> {
    match ScreenCastSession::open(parent, restore_token, cursor_mode, capture_source).await {
        Ok(session) => Ok(OpenOutcome::Opened(session)),
        Err(AshpdError::Response(ResponseError::Cancelled)) => Ok(OpenOutcome::Cancelled),
        Err(e) => Err(e),
    }
}
