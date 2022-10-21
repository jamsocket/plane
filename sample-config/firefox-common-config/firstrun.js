// Firefox shows some messages the first time it is run, but because we're running in
// an ephemeral environment, these start to get annoying. This file disables them.

pref("datareporting.policy.firstRunURL", "");
pref("datareporting.policy.dataSubmissionPolicyBypassNotification", true);
