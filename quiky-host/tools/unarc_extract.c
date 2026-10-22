#include <windows.h>
#include <stdio.h>
#include <string.h>

typedef int (__stdcall cbtype)(char*, int, int, char*);
typedef int (__cdecl *FreeArcExtract_t)(cbtype*, ...);

static int __stdcall cb(char *what, int n1, int n2, char *str) {
    if (what) {
        if (strcmp(what,"filename")==0) { printf("FILE %s\n", str?str:""); fflush(stdout); }
        else if (strcmp(what,"quit")==0) { printf("QUIT %d\n", n1); fflush(stdout); }
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc < 4) { fprintf(stderr,"usage: %s unarc.dll archive outdir\n", argv[0]); return 2; }
    char dllpath[MAX_PATH];
    if (!GetFullPathNameA(argv[1], MAX_PATH, dllpath, NULL)) return 3;
    char dir[MAX_PATH]; strcpy(dir, dllpath);
    char *p = strrchr(dir, '\\'); if (p) *p = 0;
    SetCurrentDirectoryA(dir);
    HMODULE h = LoadLibraryA(dllpath);
    if (!h) { fprintf(stderr,"LoadLibrary %s failed %lu\n", dllpath, GetLastError()); return 4; }
    FreeArcExtract_t f = (FreeArcExtract_t)GetProcAddress(h, "FreeArcExtract");
    if (!f) { fprintf(stderr,"no FreeArcExtract\n"); return 5; }
    char dp[4096]; snprintf(dp,sizeof(dp),"-dp%s",argv[3]);
    int r = f(cb, "x", "-o+", dp, argv[2], NULL);
    printf("RESULT %d\n", r); fflush(stdout);
    return r;
}
