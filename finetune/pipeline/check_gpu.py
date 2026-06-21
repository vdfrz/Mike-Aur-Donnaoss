import torch
print("torch:", torch.__version__)
print("cuda available:", torch.cuda.is_available())
print("device cap:", torch.cuda.get_device_capability())
print("device name:", torch.cuda.get_device_name(0))
x = torch.randn(8, 8, device="cuda") @ torch.randn(8, 8, device="cuda")
torch.cuda.synchronize()
print("cuda kernels ok; sample sum:", float(x.sum()))
print("bf16 supported:", torch.cuda.is_bf16_supported())
