use std::os::fd::RawFd;
use std::path::Path;

use anyhow::{Context, Result};
use async_channel::Sender;
use gstreamer as gst;
use gstreamer::prelude::*;
use gtk::glib;
use gtk4 as gtk;

use crate::ui::events::RecorderEvent;

pub fn build_video_pipeline(fd: RawFd, node_id: u32, output_path: &Path) -> Result<gst::Pipeline> {
    let pipeline = gst::Pipeline::new(Some("screen_record"));

    let src = gst::ElementFactory::make("pipewiresrc")
        .name("src")
        .property("fd", fd)
        .property("path", node_id.to_string())
        .property("do-timestamp", true)
        .build()
        .context("pipewiresrc not available (gstreamer1.0-pipewire?)")?;

    let vconv = gst::ElementFactory::make("videoconvert")
        .name("vconv")
        .build()
        .context("videoconvert missing")?;

    let vqueue = gst::ElementFactory::make("queue")
        .name("vqueue")
        .property("max-size-time", 200_000_000u64)
        .build()
        .context("queue missing")?;

    let venc = gst::ElementFactory::make("x264enc")
        .name("venc")
        .property("bitrate", 8000u32)
        .property("key-int-max", 60u32)
        .build()
        .context("x264enc missing (gstreamer1.0-plugins-ugly?)")?;
    venc.set_property_from_str("tune", "zerolatency");
    venc.set_property_from_str("speed-preset", "veryfast");

    let vparse = gst::ElementFactory::make("h264parse")
        .name("vparse")
        .build()
        .context("h264parse missing (gstreamer1.0-plugins-bad?)")?;

    let mux = gst::ElementFactory::make("matroskamux")
        .name("mux")
        .build()
        .context("matroskamux missing")?;

    let fsink = gst::ElementFactory::make("filesink")
        .name("fsink")
        .property("location", output_path.to_string_lossy().as_ref())
        .build()
        .context("filesink missing")?;

    pipeline.add_many(&[&src, &vconv, &vqueue, &venc, &vparse, &mux, &fsink])?;
    gst::Element::link_many(&[&src, &vconv, &vqueue, &venc, &vparse, &mux, &fsink])?;

    tracing::debug!(
        fd,
        node_id,
        output = %output_path.display(),
        "video pipeline built"
    );

    dump_dot(&pipeline, "pipeline-ready");

    Ok(pipeline)
}

pub fn start(pipeline: &gst::Pipeline) -> Result<()> {
    pipeline.set_state(gst::State::Playing)?;
    tracing::info!("pipeline started");
    Ok(())
}

pub fn stop_graceful(pipeline: &gst::Pipeline) {
    pipeline.send_event(gst::event::Eos::new());
    tracing::info!("EOS sent");
}

pub fn attach_bus_watch(
    pipeline: &gst::Pipeline,
    tx: Sender<RecorderEvent>,
    output_path: std::path::PathBuf,
) -> Result<glib::SourceId> {
    let bus = pipeline.bus().context("pipeline has no bus")?;
    let weak = pipeline.downgrade();
    let source_id = bus
        .add_watch(move |_, msg| {
            use gst::MessageView::*;
            match msg.view() {
                Eos(_) => {
                    tracing::info!("EOS received on bus");
                    if let Some(p) = weak.upgrade() {
                        if let Err(e) = p.set_state(gst::State::Null) {
                            tracing::warn!(%e, "failed set_state(Null) after EOS");
                        } else {
                            tracing::info!("pipeline reached Null");
                        }
                    }
                    let _ = tx.send_blocking(RecorderEvent::RecordingStopped {
                        output_path: output_path.clone(),
                    });
                }
                Error(err) => {
                    tracing::error!(
                        src = ?err.src().map(|s| s.name()),
                        error = %err.error(),
                        debug = ?err.debug(),
                        "pipeline error"
                    );
                    if let Some(p) = weak.upgrade() {
                        dump_dot(&p, "pipeline-error");
                        let _ = p.set_state(gst::State::Null);
                    }
                    let _ = tx.send_blocking(RecorderEvent::Error(err.error().to_string()));
                }
                StateChanged(sc) => {
                    if let Some(p) = weak.upgrade() {
                        if sc.src().map(|s| s.as_ptr()) == Some(p.upcast_ref::<gst::Object>().as_ptr()) {
                            tracing::debug!(
                                old = ?sc.old(),
                                new = ?sc.current(),
                                "pipeline state changed"
                            );
                            if sc.current() == gst::State::Playing {
                                dump_dot(&p, "pipeline-playing");
                                let _ = tx.send_blocking(RecorderEvent::RecordingStarted);
                            }
                        }
                    }
                }
                _ => {}
            }
            glib::Continue(true)
        })
        .context("failed to attach bus watch")?;
    Ok(source_id)
}

fn dump_dot(pipeline: &gst::Pipeline, name: &str) {
    if std::env::var_os("GST_DEBUG_DUMP_DOT_DIR").is_some() {
        gst::debug_bin_to_dot_file_with_ts(pipeline, gst::DebugGraphDetails::all(), name);
    }
}
