export function showToast(message) {
    const toast = document.getElementById("toast");
    toast.textContent = message;
    toast.classList.remove("opacity-0", "pointer-events-none");
    toast.classList.add("opacity-100");

    setTimeout(() => {
        toast.classList.remove("opacity-100");
        toast.classList.add("opacity-0", "pointer-events-none");
    }, 2000);
}