use anyhow::{Result, anyhow};
use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRule3, INetFwRules, NET_FW_ACTION_BLOCK, NET_FW_IP_PROTOCOL_ANY,
    NET_FW_PROFILE2_ALL, NET_FW_RULE_DIR_OUT, NetFwPolicy2, NetFwRule,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::core::{BSTR, Interface};

pub struct FirewallRuleGuard {
    rule_name: String,
}

impl FirewallRuleGuard {
    pub fn install_outbound_block(identity_sid: &str) -> Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() {
            return Err(anyhow!("CoInitializeEx failed: {hr:?}"));
        }

        let rule_name = format!(
            "colossal_sandbox_outbound_block_{}_{}",
            std::process::id(),
            stable_sid_suffix(identity_sid)
        );

        let result = (|| -> Result<()> {
            let policy: INetFwPolicy2 =
                unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }
                    .map_err(|err| anyhow!("CoCreateInstance NetFwPolicy2 failed: {err:?}"))?;
            let rules = unsafe { policy.Rules() }
                .map_err(|err| anyhow!("INetFwPolicy2::Rules failed: {err:?}"))?;
            ensure_block_rule(&rules, &rule_name, identity_sid)
        })();

        unsafe {
            CoUninitialize();
        }
        result?;

        Ok(Self { rule_name })
    }

    pub fn remove(self) -> Result<()> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() {
            return Err(anyhow!("CoInitializeEx failed: {hr:?}"));
        }
        let result = (|| -> Result<()> {
            let policy: INetFwPolicy2 =
                unsafe { CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER) }
                    .map_err(|err| anyhow!("CoCreateInstance NetFwPolicy2 failed: {err:?}"))?;
            let rules = unsafe { policy.Rules() }
                .map_err(|err| anyhow!("INetFwPolicy2::Rules failed: {err:?}"))?;
            remove_rule_if_present(&rules, &self.rule_name)
        })();
        unsafe {
            CoUninitialize();
        }
        result
    }
}

fn ensure_block_rule(rules: &INetFwRules, internal_name: &str, identity_sid: &str) -> Result<()> {
    let name = BSTR::from(internal_name);
    let rule: INetFwRule3 = match unsafe { rules.Item(&name) } {
        Ok(existing) => existing
            .cast()
            .map_err(|err| anyhow!("cast firewall rule to INetFwRule3 failed: {err:?}"))?,
        Err(_) => {
            let new_rule: INetFwRule3 =
                unsafe { CoCreateInstance(&NetFwRule, None, CLSCTX_INPROC_SERVER) }
                    .map_err(|err| anyhow!("CoCreateInstance NetFwRule failed: {err:?}"))?;
            unsafe { new_rule.SetName(&name) }.map_err(|err| anyhow!("SetName failed: {err:?}"))?;
            configure_rule(&new_rule, internal_name, identity_sid)?;
            unsafe { rules.Add(&new_rule) }.map_err(|err| anyhow!("Rules::Add failed: {err:?}"))?;
            new_rule
        }
    };

    configure_rule(&rule, internal_name, identity_sid)
}

fn configure_rule(rule: &INetFwRule3, internal_name: &str, identity_sid: &str) -> Result<()> {
    let local_user_spec = format!("O:LSD:(A;;CC;;;{identity_sid})");
    unsafe {
        rule.SetDescription(&BSTR::from("Colossal Sandbox Offline - Block Outbound"))
            .map_err(|err| anyhow!("SetDescription failed for {internal_name}: {err:?}"))?;
        rule.SetDirection(NET_FW_RULE_DIR_OUT)
            .map_err(|err| anyhow!("SetDirection failed for {internal_name}: {err:?}"))?;
        rule.SetAction(NET_FW_ACTION_BLOCK)
            .map_err(|err| anyhow!("SetAction failed for {internal_name}: {err:?}"))?;
        rule.SetEnabled(VARIANT_TRUE)
            .map_err(|err| anyhow!("SetEnabled failed for {internal_name}: {err:?}"))?;
        rule.SetProfiles(NET_FW_PROFILE2_ALL.0)
            .map_err(|err| anyhow!("SetProfiles failed for {internal_name}: {err:?}"))?;
        rule.SetProtocol(NET_FW_IP_PROTOCOL_ANY.0)
            .map_err(|err| anyhow!("SetProtocol failed for {internal_name}: {err:?}"))?;
        rule.SetRemoteAddresses(&BSTR::from("*"))
            .map_err(|err| anyhow!("SetRemoteAddresses failed for {internal_name}: {err:?}"))?;
        rule.SetLocalUserAuthorizedList(&BSTR::from(local_user_spec.as_str()))
            .map_err(|err| {
                anyhow!("SetLocalUserAuthorizedList failed for {internal_name}: {err:?}")
            })?;
    }
    Ok(())
}

fn remove_rule_if_present(rules: &INetFwRules, internal_name: &str) -> Result<()> {
    let name = BSTR::from(internal_name);
    if unsafe { rules.Item(&name) }.is_ok() {
        unsafe { rules.Remove(&name) }
            .map_err(|err| anyhow!("Rules::Remove failed for {internal_name}: {err:?}"))?;
    }
    Ok(())
}

fn stable_sid_suffix(identity_sid: &str) -> String {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    identity_sid.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}
