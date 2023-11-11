use std::{
    ffi::CStr,
    mem::{
        MaybeUninit,

        size_of_val, self,
    },
    ptr::NonNull,
};

use spdk_sys::{
    Errno,
    spdk_nvmf_tgt,

    to_result,

    spdk_nvmf_get_first_tgt,
    spdk_nvmf_get_next_tgt,
    spdk_nvmf_listen_opts_init,
    spdk_nvmf_subsystem_create,
    spdk_nvmf_subsystem_destroy,
    spdk_nvmf_tgt_add_transport,
    spdk_nvmf_tgt_get_name,
    spdk_nvmf_tgt_listen_ext,
};

use crate::{
    errors::{
        EINPROGRESS,
        ENOMEM,
    },
    nvme::{
        SPDK_NVME_GLOBAL_NS_TAG,

        TransportId,
    },
    nvmf::SPDK_NVMF_DISCOVERY_NQN,
    task::{
        Promise,

        complete_with_ok,
        complete_with_status,
    },
};

use super::{
    Subsystem,
    Transport,

    subsystem::{
        Subsystems,
        SubsystemType,
    },
    transport::Transports,
};

/// Represents a NVMe-oF target.
pub struct Target(NonNull<spdk_nvmf_tgt>);

unsafe impl Send for Target {}

impl Target {
    /// Returns the name of the target.
    pub fn name(&self) -> &'static CStr {
        unsafe {
            let name = spdk_nvmf_tgt_get_name(self.as_ptr());

            CStr::from_ptr(name)
        }
    }

    /// Returns a NVMf target from a raw `spdk_nvmf_target` pointer.
    pub fn from_ptr(ptr: *mut spdk_nvmf_tgt) -> Self {
        match NonNull::new(ptr) {
            Some(ptr) => Self(ptr),
            None => panic!("target pointer must not be null"),
        }
    }

    /// Returns a pointer to the underlying `spdk_nvmf_target` structure.
    pub fn as_ptr(&self) -> *mut spdk_nvmf_tgt {
        self.0.as_ptr()
    }

    /// Adds a transport to the target.
    pub async fn add_transport(&mut self, transport: Transport) -> Result<(), Errno> {
        let res = Promise::new(|cx| {
            unsafe {
                spdk_nvmf_tgt_add_transport(
                    self.as_ptr(),
                    transport.as_ptr(),
                    Some(complete_with_status),
                    cx,
                );
            }

            Ok(())
        }).await;

        match res {
            Ok(()) => {
                // The transport is now owned by the target, so we forget it to avoid
                // destroying it.
                mem::forget(transport);

                Ok(())
            },
            Err(e) => {
                // Since adding the transport failed, explicitly destroy it
                // rather then let it drop to avoid blocking current reactor from
                // executing other tasks.
                transport.destroy().await.expect("transport destroyed");

                Err(e)
            },
        }
    }

    /// Returns an iterator over the transports on this target.
    pub fn transports(&self) -> Transports {
        Transports::new(self)
    }

    /// Enables the NVMe-oF Discovery Controller subsystem on this target.
    pub fn enable_discovery(&mut self) -> Result<Subsystem, Errno> {
        let discovery = self.add_subsystem(
            SPDK_NVMF_DISCOVERY_NQN,
            SubsystemType::Discovery,
            0)?;
        
        discovery.allow_any_host(true);
        Ok(discovery)
    }

    /// Adds a subsystem to the target.
    /// 
    /// A subsystem is a collection of namespaces that are exported over
    /// NVMe-oF. It can be in one of three states: Inactive, Active, or Paused.
    /// This state affects which operations may be perform on the subsystem. On
    /// creation, the subsystem is in the Inactive state and may be activated by
    /// calling the subsystem's [`start`] method. No I/O will be processed in
    /// the Inactive or Paused states but changes to the state of the subsystem
    /// may be made.
    /// 
    /// [`start`]: method@Subsystem::start
    pub fn add_subsystem(&mut self, nqn: &CStr, subtype: SubsystemType, num_ns: u32) -> Result<Subsystem, Errno> {
        let subsys = unsafe {
            spdk_nvmf_subsystem_create(self.as_ptr(), nqn.as_ptr(), subtype.into(), num_ns)
        };

        if subsys.is_null() {
            return Err(ENOMEM);
        }

        Ok(Subsystem::from_ptr(subsys))
    }

    /// Removes a subsystem from the target.
    pub async fn remove_subsystem(&mut self, subsys: Subsystem) -> Result<(), Errno> {
        Promise::new(|cx| {
            unsafe {
                match to_result!(spdk_nvmf_subsystem_destroy(
                    subsys.as_ptr(),
                    Some(complete_with_ok),
                    cx
                )) {
                    // The subsystem was destroyed synchronously, so we must
                    // invoke the promise completion function ourselves.
                    Ok(()) => {
                        complete_with_ok(cx);
                        Ok(())
                    },
                    // The subsystem is being destroyed asynchronously.
                    Err(e) if e == EINPROGRESS => Ok(()),
                    // An subsystem is not in a state where it can be destroyed.
                    Err(e) => Err(e),
                }
            }
        }).await
    }

    /// Returns an iterator over the subsystems on this target.
    pub fn subsystems(&self) -> Subsystems {
        Subsystems::new(self)
    }

    /// Starts the subsystems on this target.
    pub async fn start_subsystems(&self) -> Result<(), Errno> {
        for subsys in self.subsystems() {
            subsys.start().await?;
        }

        Ok(())
    }

    /// Stops the subsystems on this target.
    pub async fn stop_subsystems(&self) -> Result<(), Errno> {
        for subsys in self.subsystems() {
            subsys.stop().await?;
        }

        Ok(())
    }

    /// Pauses the subsystems on this target.
    pub async fn pause_subsystems(&self) -> Result<(), Errno> {
        for subsys in self.subsystems() {
            subsys.pause(SPDK_NVME_GLOBAL_NS_TAG).await?;
        }

        Ok(())
    }

    /// Resumes the subsystems on this target.
    pub async fn resume_subsystems(&self) -> Result<(), Errno> {
        for subsys in self.subsystems() {
            subsys.resume().await?;
        }

        Ok(())
    }

    /// Begins accepting new connections on the specified transport.
    pub fn listen(&self, transport_id: &TransportId) -> Result<(), Errno> {
        unsafe {
            let mut opts = MaybeUninit::uninit();

            spdk_nvmf_listen_opts_init(opts.as_mut_ptr(), size_of_val(&opts));

            let mut opts = opts.assume_init();

            to_result!(spdk_nvmf_tgt_listen_ext(self.as_ptr(), transport_id.as_ptr(), &mut opts as *mut _))
        }
    }
}

/// An iterator over the NVMe-oF targets.
pub struct Targets(*mut spdk_nvmf_tgt);

unsafe impl Send for Targets {}

impl Iterator for Targets {
    type Item = Target;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_null() {
            None
        } else {
            unsafe {
                let tgt = self.0;

                self.0 = spdk_nvmf_get_next_tgt(tgt);

                Some(Target::from_ptr(tgt))
            }
        }
    }
}

/// Get an iterator over the NVMe-oF targets.
pub fn targets() -> Targets {
    Targets(unsafe { spdk_nvmf_get_first_tgt() })
}
