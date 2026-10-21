"""Write a transformers-style ParakeetForTDT snapshot dir (F32 model.safetensors + config/tokenizer) from Bark weights.
usage: python bark_to_hf.py <bark_dir_with_model.bark_or_model.safetensors> <hf_meta_dir> <out_dir>"""
import os, shutil, sys
import torch
from safetensors.torch import load_file, save_file
src, meta, out = sys.argv[1:4]
os.makedirs(out, exist_ok=True)
st = os.path.join(src, "model.safetensors")
if os.path.exists(st):
    sd = {k: v.to(torch.float32) for k, v in load_file(st).items()}
else:
    sys.path.insert(0, os.path.abspath(src))
    from bark_loader import load_state_dict
    sd, _ = load_state_dict(os.path.join(src, "model.bark"))
    sd = {k: v.to(torch.float32).contiguous() for k, v in sd.items()}
save_file(sd, os.path.join(out, "model.safetensors"))
for f in ("config.json", "tokenizer.json"):
    shutil.copy(os.path.join(meta, f), out)
print(out, len(sd), "tensors")
