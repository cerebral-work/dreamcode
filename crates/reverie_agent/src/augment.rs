use agent_client_protocol::{self as acp};

use crate::http::SmartContext;

/// Insert a "Relevant memory:\n<ctx>\n" text ContentBlock at position 0 of
/// the prompt's block list when memory is present and non-empty. Otherwise
/// return the blocks unchanged.
pub(crate) fn augment_prompt_blocks(
    mut blocks: Vec<acp::ContentBlock>,
    memory: Option<SmartContext>,
) -> Vec<acp::ContentBlock> {
    if let Some(ctx) = memory
        && !ctx.content.trim().is_empty()
    {
        let memory_block = acp::ContentBlock::Text(acp::TextContent::new(format!(
            "Relevant memory:\n{}\n",
            ctx.content
        )));
        blocks.insert(0, memory_block);
    }
    blocks
}
