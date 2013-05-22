#!/usr/bin/python

'''A simple implementation of LZW compression algorithm

'''

import struct

PACK_PATTERN = 'I'
PACK_SIZE = struct.calcsize(PACK_PATTERN)

class LZW(object):
	'''Compress and decompress files using LZW algorithm'''
	def __init__(self):
		super(LZW, self).__init__()

	def Compress(self, input_file, output_file):
		# Determine output file name
		#if output_file is None:
		#	output_file = input_file + '.lzw'
		#with open(input_file, 'rb') as instream, 
		#	open(output_file, 'wb') as outstream:
		#	self._Compress(instream, outstream)
		self._Compress(input_file, output_file)

	def _Compress(self, instream, outstream):
		t = EncodingTable()
		word = ''
		c = instream.read(1)
		while c:
			word += c
			if word not in t:
				prefix = word[:-1]
				suffix = word[-1]
				# emit the longest prefix of the word in the table
				outstream.write(self._Encode(t[prefix]))
				# add new word to table
				t.Add(word)
				# continue with the suffix
				word = suffix
			c = instream.read(1)
		if word:
			outstream.write(self._Encode(t[word]))

	def Decompress(self, input_file, output_file):
		#if output_file is None:
		#	last_dot = input_file.rfind('.lzw')
		#	if last_dot == -1
		#		output_file = input_file + '.wzl'
		#	else:
		#		output_file = input_file[:last_dot]
		#with open(input_file, 'rb') as instream, 
		#	open(output_file, 'wb') as outstream:
		#	self._Decompress(instream, outstream)
		self._Decompress(input_file, output_file)

	def _Decompress(self, instream, outstream):
		t = DecodingTable()
		current = instream.read(PACK_SIZE)
		if len(current) < PACK_SIZE:
			return
		curr_code = self._Decode(current)
		look_ahead = instream.read(PACK_SIZE)
		while len(look_ahead) == PACK_SIZE:
			# Output the current token - it should already be in the table
			outstream.write(t[curr_code])
			la_code = self._Decode(look_ahead)
			# In each iteration add the current word with the first letter
			# of the lookahead word
			t.Add(t[curr_code] + t[la_code][0])
			# Prepare for next iteration
			current = look_ahead
			curr_code = la_code
			look_ahead = instream.read(PACK_SIZE)
		if len(current) == PACK_SIZE:
			outstream.write(t[self._Decode(current)])

	def _Encode(self, value):
		return struct.pack(PACK_PATTERN, value)

	def _Decode(self, string):
		return struct.unpack(PACK_PATTERN, string)[0]


class LzwTable(object):
	'''Abstract base class table for LZW encoding and decoding table.'''
	def __init__(self):
		super(LzwTable, self).__init__()
		# Initialize dictionary value dispenser
		self.__value = 0
		self._d = {}
		# Initialize the table
		for i in xrange(256):
			self.Add(chr(i))

	def __contains__(self, key):
		return key in self._d

	def __getitem__(self, key):
		return self._d[key]

	def Add(self, key):
		raise NotImplementedError

	@property
	def _value(self):
		prev = self.__value
		self.__value += 1
		return prev


class EncodingTable(LzwTable):
	'''The concrete class for a symbol table for encoding with LZW.'''
	def __init__(self):
		super(EncodingTable, self).__init__()
		
	def Add(self, key):
		self._d[key] = self._value


class DecodingTable(LzwTable):
	'''The concrete class for a symbol table for decoding with LZW.'''
	def __init__(self):
		super(DecodingTable, self).__init__()
		
	def Add(self, key):
		self._d[self._value] = key


import argparse

def ParseCommandLineArgs():
	# Initialize the parser and define the command line arguments
	parser = argparse.ArgumentParser(
		description='Compress or decompress a file using the LZW algorithm.')
	group = parser.add_mutually_exclusive_group(required=True)
	group.add_argument('-c', '--compress', 
						action='store_true',
						help='Compress a file.')
	group.add_argument('-d', '--decompress', 
						action='store_true',
						help='Decompress a file.')
	parser.add_argument('input_file', 
						type=argparse.FileType('rb'),
						help='Path to the file to be read from.')
	parser.add_argument('output_file',
						type=argparse.FileType('wb'),
						help='Path to the output file.')
	# Parse the command line arguments
	args = parser.parse_args()
	return 'c' if args.compress else 'd', args.input_file, args.output_file


def main():
	op_type, input_file, output_file = ParseCommandLineArgs()
	lzw = LZW()
	if op_type == 'c':
		lzw.Compress(input_file, output_file)
	elif op_type == 'd':
		lzw.Decompress(input_file, output_file)

if __name__ == '__main__':
	main()