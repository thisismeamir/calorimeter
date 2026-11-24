/* Minimal PMPI wrapper compiled per MPI installation with `mpicc` (Spec §9.5 B23).
 * Delegates all aggregation and YAML output to `libcalo_shim_mpi.so`.
 */
#include <mpi.h>
#include <stdint.h>
#include <stdatomic.h>

extern void calo_mpi_init(int32_t rank, int32_t size, int32_t thread_level);
extern int64_t calo_mpi_now_ns(void);
extern void calo_mpi_record_call(
    const char *name,
    uint64_t comm_id,
    int64_t start_ns,
    int64_t end_ns,
    uint64_t bytes_sent,
    uint64_t bytes_recv,
    int32_t peer);
extern void calo_mpi_record_collective(
    const char *name,
    uint64_t comm_id,
    uint64_t seq,
    int64_t start_ns,
    int64_t end_ns,
    uint64_t bytes);
extern void calo_mpi_finalize(void);

static atomic_uint_fast64_t g_coll_seq = 0;

static uint64_t comm_key(MPI_Comm comm) {
    if (comm == MPI_COMM_WORLD) {
        return 0;
    }
    return (uint64_t)(uintptr_t)comm;
}

static uint64_t type_bytes(int count, MPI_Datatype dt) {
    int sz = 0;
    if (count <= 0) {
        return 0;
    }
    if (PMPI_Type_size(dt, &sz) != MPI_SUCCESS || sz <= 0) {
        return 0;
    }
    return (uint64_t)count * (uint64_t)sz;
}

int MPI_Init(int *argc, char ***argv) {
    int rc = PMPI_Init(argc, argv);
    if (rc == MPI_SUCCESS) {
        int rank = 0, size = 1;
        PMPI_Comm_rank(MPI_COMM_WORLD, &rank);
        PMPI_Comm_size(MPI_COMM_WORLD, &size);
        calo_mpi_init(rank, size, MPI_THREAD_SINGLE);
    }
    return rc;
}

int MPI_Init_thread(int *argc, char ***argv, int required, int *provided) {
    int rc = PMPI_Init_thread(argc, argv, required, provided);
    if (rc == MPI_SUCCESS) {
        int rank = 0, size = 1;
        PMPI_Comm_rank(MPI_COMM_WORLD, &rank);
        PMPI_Comm_size(MPI_COMM_WORLD, &size);
        calo_mpi_init(rank, size, provided ? *provided : MPI_THREAD_SINGLE);
    }
    return rc;
}

int MPI_Finalize(void) {
    calo_mpi_finalize();
    return PMPI_Finalize();
}

int MPI_Send(const void *buf, int count, MPI_Datatype dt, int dest, int tag, MPI_Comm comm) {
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Send(buf, count, dt, dest, tag, comm);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_call("MPI_Send", comm_key(comm), t0, t1, type_bytes(count, dt), 0, dest);
    return rc;
}

int MPI_Recv(void *buf, int count, MPI_Datatype dt, int src, int tag, MPI_Comm comm, MPI_Status *st) {
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Recv(buf, count, dt, src, tag, comm, st);
    int64_t t1 = calo_mpi_now_ns();
    int peer = (st && st != MPI_STATUS_IGNORE) ? st->MPI_SOURCE : src;
    calo_mpi_record_call("MPI_Recv", comm_key(comm), t0, t1, 0, type_bytes(count, dt), peer);
    return rc;
}

int MPI_Isend(const void *buf, int count, MPI_Datatype dt, int dest, int tag, MPI_Comm comm, MPI_Request *req) {
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Isend(buf, count, dt, dest, tag, comm, req);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_call("MPI_Isend", comm_key(comm), t0, t1, type_bytes(count, dt), 0, dest);
    return rc;
}

int MPI_Irecv(void *buf, int count, MPI_Datatype dt, int src, int tag, MPI_Comm comm, MPI_Request *req) {
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Irecv(buf, count, dt, src, tag, comm, req);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_call("MPI_Irecv", comm_key(comm), t0, t1, 0, type_bytes(count, dt), src);
    return rc;
}

int MPI_Sendrecv(const void *sbuf, int scount, MPI_Datatype sdt, int dest, int stag,
                 void *rbuf, int rcount, MPI_Datatype rdt, int src, int rtag,
                 MPI_Comm comm, MPI_Status *st) {
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Sendrecv(sbuf, scount, sdt, dest, stag, rbuf, rcount, rdt, src, rtag, comm, st);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_call("MPI_Sendrecv", comm_key(comm), t0, t1,
                         type_bytes(scount, sdt), type_bytes(rcount, rdt), dest);
    return rc;
}

int MPI_Wait(MPI_Request *req, MPI_Status *st) {
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Wait(req, st);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_call("MPI_Wait", 0, t0, t1, 0, 0, -1);
    return rc;
}

int MPI_Waitall(int count, MPI_Request reqs[], MPI_Status sts[]) {
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Waitall(count, reqs, sts);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_call("MPI_Waitall", 0, t0, t1, 0, 0, -1);
    return rc;
}

int MPI_Barrier(MPI_Comm comm) {
    uint64_t seq = atomic_fetch_add(&g_coll_seq, 1);
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Barrier(comm);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_collective("MPI_Barrier", comm_key(comm), seq, t0, t1, 0);
    return rc;
}

int MPI_Bcast(void *buf, int count, MPI_Datatype dt, int root, MPI_Comm comm) {
    uint64_t seq = atomic_fetch_add(&g_coll_seq, 1);
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Bcast(buf, count, dt, root, comm);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_collective("MPI_Bcast", comm_key(comm), seq, t0, t1, type_bytes(count, dt));
    return rc;
}

int MPI_Allreduce(const void *sbuf, void *rbuf, int count, MPI_Datatype dt, MPI_Op op, MPI_Comm comm) {
    uint64_t seq = atomic_fetch_add(&g_coll_seq, 1);
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Allreduce(sbuf, rbuf, count, dt, op, comm);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_collective("MPI_Allreduce", comm_key(comm), seq, t0, t1, type_bytes(count, dt));
    return rc;
}

int MPI_Allgather(const void *sbuf, int scount, MPI_Datatype sdt,
                  void *rbuf, int rcount, MPI_Datatype rdt, MPI_Comm comm) {
    uint64_t seq = atomic_fetch_add(&g_coll_seq, 1);
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Allgather(sbuf, scount, sdt, rbuf, rcount, rdt, comm);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_collective("MPI_Allgather", comm_key(comm), seq, t0, t1, type_bytes(scount, sdt));
    return rc;
}

int MPI_Alltoall(const void *sbuf, int scount, MPI_Datatype sdt,
                 void *rbuf, int rcount, MPI_Datatype rdt, MPI_Comm comm) {
    uint64_t seq = atomic_fetch_add(&g_coll_seq, 1);
    int64_t t0 = calo_mpi_now_ns();
    int rc = PMPI_Alltoall(sbuf, scount, sdt, rbuf, rcount, rdt, comm);
    int64_t t1 = calo_mpi_now_ns();
    calo_mpi_record_collective("MPI_Alltoall", comm_key(comm), seq, t0, t1, type_bytes(scount, sdt));
    return rc;
}
