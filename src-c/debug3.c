// Copyright (c) 2016 Fabian Schuiki
#include <llhd.h>
#include <stdint.h>
#include <assert.h>
#include <stdio.h>
#include <string.h>

void llhd_desequentialize(llhd_value_t proc);

int main() {
	llhd_module_t M;
	llhd_value_t E, P, I, Q, BBentry, BBckl, BBckla, BBcklb, BBckh, k0;
	llhd_type_t Ety, Pty, i1ty;

	i1ty = llhd_type_new_int(1);
	M = llhd_module_new("debug3");

	Ety = llhd_type_new_comp((llhd_type_t[]){i1ty,i1ty}, 2, (llhd_type_t[]){i1ty}, 1);
	E = llhd_entity_new(Ety, "LAGCE");
	llhd_value_set_name(llhd_unit_get_input(E,0), "CK");
	llhd_value_set_name(llhd_unit_get_input(E,1), "E");
	llhd_value_set_name(llhd_unit_get_output(E,0), "GCK");
	llhd_type_unref(Ety);
	llhd_unit_append_to(E,M);
	llhd_value_unref(E);

	Pty = llhd_type_new_comp((llhd_type_t[]){i1ty,i1ty,i1ty}, 3, (llhd_type_t[]){i1ty,i1ty}, 2);
	P = llhd_proc_new(Pty, "LAGCE_proc");
	llhd_value_set_name(llhd_unit_get_input(P,0), "CK");
	llhd_value_set_name(llhd_unit_get_input(P,1), "E");
	llhd_value_set_name(llhd_unit_get_input(P,2), "Q");
	llhd_value_set_name(llhd_unit_get_output(P,0), "GCK");
	llhd_value_set_name(llhd_unit_get_output(P,1), "Q");
	llhd_type_unref(Pty);
	llhd_unit_append_to(P,M);
	llhd_value_unref(P);

	Q = llhd_inst_sig_new(i1ty, "Q");
	llhd_inst_append_to(Q, E);
	llhd_value_unref(Q);
	I = llhd_inst_instance_new(P,
		(llhd_value_t[]){
			llhd_unit_get_input(E,0),
			llhd_unit_get_input(E,1),
			Q
		}, 3,
		(llhd_value_t[]){
			llhd_unit_get_output(E,0),
			Q
		}, 2,
		"p"
	);
	llhd_inst_append_to(I, E);
	llhd_value_unref(I);

	BBentry = llhd_block_new("entry");
	BBckl = llhd_block_new("ckl");
	BBckla = llhd_block_new("ckla");
	BBcklb = llhd_block_new("cklb");
	BBckh = llhd_block_new("ckh");
	llhd_block_append_to(BBentry,P);
	llhd_block_append_to(BBckl,P);
	llhd_block_append_to(BBckla,P);
	llhd_block_append_to(BBcklb,P);
	llhd_block_append_to(BBckh,P);
	llhd_value_unref(BBentry);
	llhd_value_unref(BBckl);
	llhd_value_unref(BBckh);
	llhd_value_unref(BBckla);
	llhd_value_unref(BBcklb);

	k0 = llhd_const_int_new(0);
	I = llhd_inst_compare_new(LLHD_CMP_EQ, llhd_unit_get_input(P,0), k0, NULL);
	llhd_value_unref(k0);
	llhd_inst_append_to(I, BBentry);
	llhd_value_unref(I);
	I = llhd_inst_branch_new_cond(I, BBckl, BBckh);
	llhd_inst_append_to(I, BBentry);
	llhd_value_unref(I);

	// I = llhd_inst_drive_new(llhd_unit_get_output(P,1), llhd_unit_get_input(P,1));
	// llhd_inst_append_to(I, BBckl);
	// llhd_value_unref(I);
	k0 = llhd_const_int_new(0);
	I = llhd_inst_drive_new(llhd_unit_get_output(P,0), k0);
	llhd_value_unref(k0);
	llhd_inst_append_to(I, BBckl);
	llhd_value_unref(I);

	k0 = llhd_const_int_new(0);
	I = llhd_inst_compare_new(LLHD_CMP_EQ, llhd_unit_get_input(P,2), k0, NULL);
	llhd_value_unref(k0);
	llhd_inst_append_to(I, BBckl);
	llhd_value_unref(I);
	I = llhd_inst_branch_new_cond(I, BBckla, BBcklb);
	llhd_inst_append_to(I, BBckl);
	llhd_value_unref(I);

	// I = llhd_inst_ret_new();
	// llhd_inst_append_to(I, BBckl);
	// llhd_value_unref(I);

	I = llhd_inst_drive_new(llhd_unit_get_output(P,1), llhd_unit_get_input(P,0));
	llhd_inst_append_to(I, BBckla);
	llhd_value_unref(I);
	I = llhd_inst_ret_new();
	llhd_inst_append_to(I, BBckla);
	llhd_value_unref(I);
	I = llhd_inst_drive_new(llhd_unit_get_output(P,1), llhd_unit_get_input(P,1));
	llhd_inst_append_to(I, BBcklb);
	llhd_value_unref(I);
	I = llhd_inst_ret_new();
	llhd_inst_append_to(I, BBcklb);
	llhd_value_unref(I);

	I = llhd_inst_drive_new(llhd_unit_get_output(P,0), llhd_unit_get_input(P,2));
	llhd_inst_append_to(I, BBckh);
	llhd_value_unref(I);
	I = llhd_inst_ret_new();
	llhd_inst_append_to(I, BBckh);
	llhd_value_unref(I);

	// llhd_value_unref(E);
	// llhd_value_unref(P);
	llhd_type_unref(i1ty);

	llhd_asm_write_module(M, stdout);
	printf("\n===== DESEQUENTIALIZE =====\n");
	llhd_desequentialize(P);
	printf("===== DONE =====\n\n");
	llhd_asm_write_module(M, stdout);

	llhd_module_free(M);

	return 0;
}